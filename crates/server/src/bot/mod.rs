//! Server-only bot management and the look → plan → emit AI loop.
//!
//! The brain itself lives in the `bots` crate; this module owns the ECS side:
//! the roster, scheduling, spawning and the `BotDebug` writer.
//! `bot_brain_input` stays in the FixedUpdate chain before `simulate_players`
//! and writes every alive bot's `ActionState` each tick.

pub(crate) mod debug;

use std::path::Path;

use bevy::prelude::*;
use bots::Brain;
use bots::config::BotBuild;
use bots::plan::Action;
use bots::schedule::Tier;
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

/// The builds the server seats, one bot each.
#[derive(Resource)]
pub struct BotRoster(pub Vec<BotBuild>);

/// The roster from `BOT_ROSTER` (a roster file), else the embedded default.
pub fn roster_from_env(get: &dyn Fn(&str) -> Option<String>) -> Result<BotRoster, String> {
    match get("BOT_ROSTER").filter(|path| !path.trim().is_empty()) {
        Some(path) => {
            let builds = bots::roster::load(Path::new(path.trim()))?;
            info!(path = %path, bots = builds.len(), "Bot roster loaded from BOT_ROSTER");
            Ok(BotRoster(builds))
        }
        None => {
            let builds = bots::roster::builtin()?;
            info!(bots = builds.len(), "Bot roster: built-in default");
            Ok(BotRoster(builds))
        }
    }
}

/// Spawn one bot per build of the roster.
pub fn setup_bots(mut commands: Commands, roster: Res<BotRoster>) {
    let builds = roster.0.clone();
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

/// Look → plan → emit for every alive bot. A bot looks on its reaction
/// cadence (one `wants_view` predicate) and whenever a planned turn is due;
/// the snapshot is built once per tick in which any bot looks. Turns are
/// one-shot: the brain emits each planned turn on its own tick, and every
/// other tick the bot holds straight (`PlayerInput::None`). Re-emitting a
/// cached turn every tick would spin the rider (Left, Left = 180° into its own
/// trail).
pub fn bot_brain_input(
    mut commands: Commands,
    debug_cfg: Res<BotDebugConfig>,
    scheduler: Res<SchedulerConfig>,
    mut manager: ResMut<BotManager>,
    builder: WorldSnapshotBuilder,
    mut bots: Query<
        (
            Entity,
            &mut BotBrain,
            &IsAlive,
            &mut ActionState<PlayerInput>,
        ),
        With<Player>,
    >,
) {
    let tick = manager.tick;
    manager.tick = manager.tick.wrapping_add(1);

    let looking: Vec<Entity> = bots
        .iter()
        .filter(|(_, brain, alive, _)| alive.0 && brain.brain.wants_view(tick, &scheduler))
        .map(|(entity, ..)| entity)
        .collect();
    let snapshot = (!looking.is_empty()).then(|| builder.build(tick));
    let alive_riders = snapshot
        .as_ref()
        .map(|world| world.riders.iter().filter(|r| r.alive).count())
        .unwrap_or(0);

    for (entity, mut brain, _, mut input) in bots.iter_mut() {
        let mut action = Action::Hold;
        if looking.contains(&entity)
            && let Some(world) = snapshot.as_ref()
            && let Some(index) = world.index_of(entity.to_bits())
        {
            let tier = Tier::from_load(alive_riders, &scheduler);
            let decision = brain.brain.act(world, index, tier, &scheduler);
            if let Some(report) = &decision.report {
                debug!(
                    bot = %brain.brain.build.name,
                    kind = ?report.kind,
                    mode = ?report.mode,
                    action = ?decision.action,
                    area = report.area,
                    open = report.open,
                    scores = ?report.scores,
                    "bot plan"
                );
                if debug_cfg.enabled {
                    let snapshot = debug::build_debug(
                        report,
                        world,
                        index,
                        decision.action,
                        &brain.brain.build.knobs,
                    );
                    debug::insert_if_enabled(&mut commands, &debug_cfg, entity, snapshot);
                }
            }
            action = decision.action;
        }
        input.0 = action.to_input();
    }
}

/// Auto-cycle after `CYCLE_DURATION_TICKS`; respawn dead bots after
/// `RESPAWN_DELAY_TICKS`, preserving each slot's build.
pub fn bot_spawner(
    mut commands: Commands,
    mut manager: ResMut<BotManager>,
    bots: Query<(Entity, &IsAlive), (With<BotBrain>, With<Player>)>,
    rounds: Option<Res<crate::rounds::MatchState>>,
) {
    // Rounds mode: the round controller places the bots. No respawns, and no
    // cycle that would replace a bot (and its score) in the middle of a match.
    let rounds_run = rounds.is_some_and(|state| state.is_running());
    let BotManager {
        slots,
        builds,
        next_bot_number,
        tick: _,
    } = &mut *manager;
    let mut number = *next_bot_number;

    for (index, slot) in slots.iter_mut().enumerate() {
        slot.ticks_alive += 1;
        if rounds_run {
            slot.respawn_ticks = None;
            continue;
        }

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
/// the writer overwrites it on the next planning round.
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
                brain: Brain::new(build.clone(), rand::random(), slot_index),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_is_the_built_in_one_unless_a_file_is_named() {
        let none = |_: &str| None;
        let roster = roster_from_env(&none).unwrap();
        assert!(roster.0.len() >= 5);
        let missing = |_: &str| Some("no/such/roster.toml".to_owned());
        assert!(roster_from_env(&missing).is_err());
    }
}
