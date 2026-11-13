//! Server-only bot management and the sense → decide → emit AI loop.
//!
//! The brain itself lives in the `bots` crate; this module owns the ECS side:
//! the roster, scheduling, spawning and the `BotDebug` writer.
//! `bot_brain_input` stays in the FixedUpdate chain before `simulate_players`
//! and always writes every alive bot's `ActionState`, caching between epochs.

pub(crate) mod debug;

use bevy::prelude::*;
use bots::Brain;
use bots::config::{BotBuild, default_builds};
use bots::forecast::PhysicsForecaster;
use bots::schedule::{Tier, should_redecide};
use bots::strategy::{Action, strategy_for};
use bots::world::WorldSnapshotBuilder;
use lightyear::prelude::*;
use shared::{
    ActionState, Direction, DisplayName, IsAlive, Player, PlayerColor, PlayerInput, Position,
    SpeedMult, Trail,
};

pub use bots::schedule::SchedulerConfig;
pub use debug::BotDebugConfig;

const CYCLE_DURATION_TICKS: u32 = 3600;
const RESPAWN_DELAY_TICKS: u32 = 240;
const SPAWN_MARGIN: f32 = 200.0;

const BOT_COLORS: [u32; 8] = [
    0xFF4444, 0x44FF44, 0x4488FF, 0xFFCC00, 0xFF44FF, 0x44FFFF, 0xFF8800, 0x8844FF,
];

/// Per-bot AI state. Survives only as long as the entity; auto-cycle and
/// respawn recreate it from the slot's `BotBuild`.
#[derive(Component)]
pub struct BotBrain {
    pub brain: Brain,
    /// Roster slot; phase-shifts this bot's decision epochs.
    pub slot_index: usize,
}

#[derive(Resource)]
pub struct BotManager {
    slots: Vec<BotSlot>,
    builds: Vec<BotBuild>,
    next_bot_number: u32,
    tick: u32,
}

struct BotSlot {
    entity: Option<Entity>,
    bot_number: u32,
    build_index: usize,
    ticks_alive: u32,
    respawn_ticks: Option<u32>,
}

/// Spawn the heterogeneous roster from `default_builds`.
pub fn setup_bots(mut commands: Commands) {
    let builds = default_builds();
    let slots = builds
        .iter()
        .enumerate()
        .map(|(index, _)| BotSlot {
            entity: None,
            bot_number: (index + 1) as u32,
            build_index: index,
            ticks_alive: 0,
            respawn_ticks: None,
        })
        .collect();
    let next_bot_number = (builds.len() + 1) as u32;
    let mut manager = BotManager {
        slots,
        builds,
        next_bot_number,
        tick: 0,
    };
    for index in 0..manager.slots.len() {
        let build = manager.builds[manager.slots[index].build_index].clone();
        let number = manager.slots[index].bot_number;
        let entity = spawn_bot_entity(&mut commands, number, &build, index);
        manager.slots[index].entity = Some(entity);
    }
    commands.insert_resource(manager);
}

/// Sense → decide → emit for every alive bot. Decisions are scheduled per bot
/// (one `should_redecide` predicate). Turns are one-shot: the decided action
/// is emitted only on its redecide tick; between epochs bots hold straight
/// (`PlayerInput::None`). Re-emitting a cached turn every tick would spin the
/// rider (Left, Left = 180° into its own trail).
#[allow(clippy::type_complexity)]
pub fn bot_brain_input(
    mut commands: Commands,
    debug_cfg: Res<BotDebugConfig>,
    scheduler: Res<SchedulerConfig>,
    mut manager: ResMut<BotManager>,
    builder: WorldSnapshotBuilder,
    mut bots: Query<
        (Entity, &mut BotBrain, &mut ActionState<PlayerInput>),
        (With<Player>, With<IsAlive>),
    >,
) {
    let tick = manager.tick;
    manager.tick = manager.tick.wrapping_add(1);

    let decisions: Vec<(Entity, bool)> = bots
        .iter()
        .map(|(entity, brain, _)| {
            (
                entity,
                should_redecide(
                    &scheduler,
                    tick,
                    brain.slot_index,
                    brain.brain.build.knobs.reaction_ticks,
                ),
            )
        })
        .collect();

    // One snapshot per tick in which any bot redecides.
    let snapshot = decisions
        .iter()
        .any(|(_, redecide)| *redecide)
        .then(|| builder.build(tick));
    let alive_riders = snapshot
        .as_ref()
        .map(|world| world.riders.iter().filter(|r| r.alive).count())
        .unwrap_or(0);

    for (entity, mut brain, mut input) in bots.iter_mut() {
        let redecide = decisions
            .iter()
            .find(|(candidate, _)| *candidate == entity)
            .is_some_and(|(_, redecide)| *redecide);
        // One-shot: only a fresh decision turns. `brain.cached` stays as the
        // last decision for observability but is never re-emitted.
        let mut fresh: Option<Action> = None;
        if redecide && let Some(world) = snapshot.as_ref() {
            let tier = Tier::from_load(alive_riders, &scheduler);
            if let Some(index) = world.index_of(entity.to_bits()) {
                let bots::Decision {
                    action,
                    perception,
                    knobs,
                } = brain.brain.think(world, index, tier);
                let strategy = strategy_for(brain.brain.build.personality);
                // Re-score for the debug snapshot (same inputs `decide`
                // argmaxed over); skipped entirely when the gate is off.
                if debug_cfg.enabled {
                    let scores = strategy.score(&perception, world, &PhysicsForecaster, &knobs);
                    let foe = world.riders.iter().enumerate().find_map(|(i, r)| {
                        (i != perception.rider && r.alive).then_some((r.pos, r.dir))
                    });
                    let (tx, ty) = foe.map_or((0.0, 0.0), |(p, _)| (p.x, p.y));
                    debug!(
                        bot = %strategy.name(),
                        mode = ?strategy.mode(&perception),
                        ?action,
                        freedom = perception.freedom,
                        threat = perception.threat,
                        scores = ?scores,
                        aggression = knobs.aggression,
                        caution = knobs.caution,
                        greed = knobs.greed,
                        lookahead = knobs.lookahead_ticks,
                        reaction = knobs.reaction_ticks,
                        foe_x = tx,
                        foe_y = ty,
                        turn_prob = perception.opponent_turn_prob,
                        "bot decision"
                    );
                    let snapshot =
                        debug::build_debug(strategy, &perception, scores, world, action, &knobs);
                    debug::insert_if_enabled(&mut commands, &debug_cfg, entity, snapshot);
                } else {
                    debug!(
                        bot = %strategy.name(),
                        mode = ?strategy.mode(&perception),
                        ?action,
                        freedom = perception.freedom,
                        threat = perception.threat,
                        "bot decision"
                    );
                }
                fresh = Some(action);
            }
        }
        input.0 = fresh.map_or(PlayerInput::None, Action::to_input);
    }
}

/// Auto-cycle after `CYCLE_DURATION_TICKS`; respawn dead bots after
/// `RESPAWN_DELAY_TICKS`, preserving each slot's build.
#[allow(clippy::type_complexity)]
pub fn bot_spawner(
    mut commands: Commands,
    mut manager: ResMut<BotManager>,
    bots: Query<(Entity, &IsAlive), (With<BotBrain>, With<Player>)>,
) {
    let BotManager {
        slots,
        builds,
        next_bot_number,
        tick: _,
    } = &mut *manager;
    let mut number = *next_bot_number;

    for (index, slot) in slots.iter_mut().enumerate() {
        slot.ticks_alive += 1;

        // Full cycle: despawn old entity, replace with a fresh bot number.
        if slot.ticks_alive >= CYCLE_DURATION_TICKS {
            if let Some(entity) = slot.entity {
                commands.entity(entity).despawn();
            }
            let assigned = number;
            number += 1;
            let entity =
                spawn_bot_entity(&mut commands, assigned, &builds[slot.build_index], index);
            slot.entity = Some(entity);
            slot.bot_number = assigned;
            slot.ticks_alive = 0;
            slot.respawn_ticks = None;
            continue;
        }

        let Some(entity) = slot.entity else {
            continue;
        };
        let Ok((_, alive)) = bots.get(entity) else {
            continue;
        };
        if !alive.0 && slot.respawn_ticks.is_none() {
            slot.respawn_ticks = Some(RESPAWN_DELAY_TICKS);
        }
        if let Some(ref mut ticks) = slot.respawn_ticks {
            *ticks = ticks.saturating_sub(1);
            if *ticks == 0 {
                commands.entity(entity).despawn();
                let new_entity = spawn_bot_entity(
                    &mut commands,
                    slot.bot_number,
                    &builds[slot.build_index],
                    index,
                );
                slot.entity = Some(new_entity);
                slot.respawn_ticks = None;
            }
        }
    }

    *next_bot_number = number;
}

/// Spawn a bot `Player` entity at a random position with the given number.
///
/// Inserts a default [`shared::BotDebug`] only when `TRON_BOT_DEBUG` is truthy;
/// the writer overwrites it on the next decision epoch.
fn spawn_bot_entity(
    commands: &mut Commands,
    bot_number: u32,
    build: &BotBuild,
    slot_index: usize,
) -> Entity {
    let color = BOT_COLORS[(bot_number - 1) as usize % BOT_COLORS.len()];
    let (pos, dir) = random_spawn();

    let entity = commands
        .spawn((
            Player,
            BotBrain {
                brain: Brain::new(build.clone()),
                slot_index,
            },
            shared::PlayerId(format!("bot-{}", bot_number)),
            DisplayName(build.name.clone()),
            Position(pos),
            Direction(dir),
            SpeedMult::base(),
            PlayerColor(color),
            IsAlive(true),
            Trail::new(pos),
            shared::ShouldHandleDeath(true),
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    if debug::enabled_from_env() {
        commands.entity(entity).insert(shared::BotDebug::default());
    }
    entity
}

/// Pick a random spawn position (with `SPAWN_MARGIN` from walls) and a cardinal
/// direction.
fn random_spawn() -> (Vec2, Vec2) {
    let half: f32 = shared::ARENA_WIDTH * 0.5;
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
