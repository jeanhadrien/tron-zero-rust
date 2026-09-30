//! Bot management and AI for headless opponents.
//!
//! Bots drive themselves with random turn decisions, auto-cycle every 30s,
//! and respawn 2s after death.

use bevy::prelude::*;
use lightyear::prelude::*;

use shared::{
    self, ActionState, Direction, IsAlive, Player, PlayerColor, PlayerInput, Position, SpeedMult,
    Trail, Velocity,
};

const CYCLE_DURATION_TICKS: u32 = 3600;
const RESPAWN_DELAY_TICKS: u32 = 240;
const TURN_DECISION_MIN_TICKS: u32 = 15;
const TURN_DECISION_MAX_TICKS: u32 = 30;
const SPAWN_MARGIN: f32 = 200.0;

const BOT_COLORS: [u32; 8] = [
    0xFF4444, 0x44FF44, 0x4488FF, 0xFFCC00, 0xFF44FF, 0x44FFFF, 0xFF8800, 0x8844FF,
];

#[derive(Component)]
pub struct BotBrain {
    pub turn_timer: u32,
}

#[derive(Resource)]
pub struct BotManager {
    slots: [BotSlot; 3],
    next_bot_number: u32,
}

struct BotSlot {
    entity: Option<Entity>,
    bot_number: u32,
    ticks_alive: u32,
    respawn_ticks: Option<u32>,
}

/// Spawn 3 bot lightcycles at random positions on startup.
pub fn setup_bots(mut commands: Commands) {
    let mut manager = BotManager {
        slots: [
            BotSlot {
                entity: None,
                bot_number: 1,
                ticks_alive: 0,
                respawn_ticks: None,
            },
            BotSlot {
                entity: None,
                bot_number: 2,
                ticks_alive: 0,
                respawn_ticks: None,
            },
            BotSlot {
                entity: None,
                bot_number: 3,
                ticks_alive: 0,
                respawn_ticks: None,
            },
        ],
        next_bot_number: 4,
    };

    for i in 0..3 {
        let entity = spawn_bot_entity(&mut commands, (i + 1) as u32);
        manager.slots[i].entity = Some(entity);
    }

    commands.insert_resource(manager);
}

/// Write random turn decisions into each alive bot's `ActionState` every
/// `TURN_DECISION_MIN_TICKS..=TURN_DECISION_MAX_TICKS` ticks.
///
/// Always sets `ActionState.0` each tick so shared simulation (which reads
/// immutably) sees the correct value. On non-decision ticks, writes `None`.
///
/// Runs before shared simulation so the turn takes effect this tick.
#[allow(clippy::type_complexity)]
pub fn bot_brain_input(
    mut bots: Query<(&mut BotBrain, &mut ActionState<PlayerInput>), (With<Player>, With<IsAlive>)>,
) {
    for (mut brain, mut input) in &mut bots {
        brain.turn_timer = brain.turn_timer.saturating_sub(1);
        if brain.turn_timer == 0 {
            let turn = if rand::random::<bool>() {
                PlayerInput::TurnLeft
            } else {
                PlayerInput::TurnRight
            };
            input.0 = turn;
            brain.turn_timer = (rand::random::<u32>()
                % (TURN_DECISION_MAX_TICKS - TURN_DECISION_MIN_TICKS + 1))
                + TURN_DECISION_MIN_TICKS;
        } else {
            input.0 = PlayerInput::None;
        }
    }
}

/// Handle bot lifecycle: auto-cycle after `CYCLE_DURATION_TICKS`, respawn dead
/// bots after `RESPAWN_DELAY_TICKS`.
///
/// Runs after shared simulation so death flags are current.
#[allow(clippy::type_complexity)]
pub fn bot_spawner(
    mut commands: Commands,
    mut manager: ResMut<BotManager>,
    bots: Query<(Entity, &IsAlive), (With<BotBrain>, With<Player>)>,
) {
    let mut next_number = manager.next_bot_number;

    for slot in &mut manager.slots {
        slot.ticks_alive += 1;

        // Full cycle: despawn old entity, replace with a fresh bot number.
        if slot.ticks_alive >= CYCLE_DURATION_TICKS {
            if let Some(entity) = slot.entity {
                commands.entity(entity).despawn();
            }
            let number = next_number;
            next_number += 1;
            let entity = spawn_bot_entity(&mut commands, number);
            slot.entity = Some(entity);
            slot.bot_number = number;
            slot.ticks_alive = 0;
            slot.respawn_ticks = None;
            continue;
        }

        // Not yet cycled — check alive status.
        let Some(entity) = slot.entity else {
            continue;
        };
        let Ok((_, alive)) = bots.get(entity) else {
            continue;
        };

        if !alive.0 {
            // Dead — start respawn countdown if not already counting.
            if slot.respawn_ticks.is_none() {
                slot.respawn_ticks = Some(RESPAWN_DELAY_TICKS);
            }
        }

        // Decrement respawn timer. When it reaches 0, respawn with same number.
        if let Some(ref mut ticks) = slot.respawn_ticks {
            *ticks = ticks.saturating_sub(1);
            if *ticks == 0 {
                commands.entity(entity).despawn();
                let new_entity = spawn_bot_entity(&mut commands, slot.bot_number);
                slot.entity = Some(new_entity);
                slot.respawn_ticks = None;
            }
        }
    }

    manager.next_bot_number = next_number;
}

/// Spawn a bot `Player` entity bundle at a random position with the given number.
fn spawn_bot_entity(commands: &mut Commands, bot_number: u32) -> Entity {
    let color = BOT_COLORS[(bot_number - 1) as usize % BOT_COLORS.len()];
    let (pos, dir) = random_spawn();

    commands
        .spawn((
            Player,
            BotBrain { turn_timer: 10 },
            shared::PlayerId(format!("bot-{}", bot_number)),
            Position(pos),
            Direction(dir),
            Velocity(Vec2::ZERO),
            SpeedMult::base(),
            PlayerColor(color),
            IsAlive(true),
            Trail::new(pos),
            shared::ShouldHandleDeath(true),
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id()
}

/// Pick a random spawn position (with `SPAWN_MARGIN` from walls) and a cardinal
/// direction.
fn random_spawn() -> (Vec2, Vec2) {
    let half: f32 = 1200.0;
    let low = -half + SPAWN_MARGIN;
    let high = half - SPAWN_MARGIN;

    let x = low + rand::random::<f32>() * (high - low);
    let y = low + rand::random::<f32>() * (high - low);
    let pos = Vec2::new(x, y);

    let dir = match rand::random::<u32>() % 4 {
        0 => Vec2::new(1.0, 0.0),
        1 => Vec2::new(-1.0, 0.0),
        2 => Vec2::new(0.0, 1.0),
        _ => Vec2::new(0.0, -1.0),
    };

    (pos, dir)
}
