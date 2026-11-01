//! Server-only bot management and the sense → decide → emit AI loop.
//!
//! `bot_brain_input` stays in the FixedUpdate chain before `simulate_players`
//! and always writes every alive bot's `ActionState`, caching between epochs.

pub(crate) mod config;
pub(crate) mod debug;
pub(crate) mod forecast;
pub(crate) mod memory;
pub(crate) mod perceive;
pub(crate) mod schedule;
pub(crate) mod strategy;
pub(crate) mod territory;
pub(crate) mod world;

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::{
    ActionState, Direction, DisplayName, IsAlive, Player, PlayerColor, PlayerInput, Position,
    SpeedMult, Trail, Velocity,
};

use config::{BotBuild, default_builds};
use forecast::PhysicsForecaster;
use memory::BotMemory;
use perceive::{RaySensor, Sensor};
use schedule::{Tier, should_redecide};
use strategy::{Action, decide, strategy_for};
use world::WorldSnapshotBuilder;

pub use debug::BotDebugConfig;
pub use schedule::SchedulerConfig;

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
    pub build: BotBuild,
    pub memory: BotMemory,
    pub cached: Action,
    pub epoch: u32,
    pub degradation: Tier,
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
                    brain.build.knobs.reaction_ticks,
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
            brain.degradation = tier;
            if let Some(index) = world.index_of(entity.to_bits()) {
                let self_id = world.riders[index].id;
                for rider in &world.riders {
                    if rider.id != self_id && rider.alive {
                        brain.memory.opponent.observe(rider.id, rider.dir);
                    }
                }
                let sensor = RaySensor {
                    rider: index,
                    bfs_budget: brain.build.knobs.bfs_budget.min(tier.max_bfs()),
                };
                let mut perception = sensor.sense(world);
                brain.memory.observe(perception.freedom, perception.exits);
                perception.memory_trend = brain.memory.trend();
                perception.opponent_turn_prob = brain.memory.opponent.turn_prob();
                let forecaster = PhysicsForecaster;
                let strategy = strategy_for(brain.build.personality);
                // Clamp the Elo horizon by the LOD tier before strategies see it.
                let mut knobs = brain.build.knobs.clone();
                knobs.lookahead_ticks = knobs.lookahead_ticks.min(tier.max_lookahead());
                let action = decide(strategy, &perception, world, &forecaster, &knobs);
                // Re-score for the debug snapshot (same inputs `decide`
                // argmaxed over); skipped entirely when the gate is off.
                if debug_cfg.enabled {
                    let scores = strategy.score(&perception, world, &forecaster, &knobs);
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
                brain.cached = action;
                fresh = Some(action);
                brain.epoch = brain.epoch.wrapping_add(1);
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

/// Pure bot label: `{strategy}-{elo} #{N}` (e.g. `trapper-2500 #3`).
/// Display-only; `PlayerId` stays the stable identity.
pub(crate) fn format_bot_name(build: &BotBuild, bot_number: u32) -> String {
    format!(
        "{}-{} #{}",
        strategy_for(build.personality).name(),
        build.elo.0,
        bot_number
    )
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
                build: build.clone(),
                memory: BotMemory::default(),
                cached: Action::Hold,
                epoch: 0,
                degradation: Tier::Full,
                slot_index,
            },
            shared::PlayerId(format!("bot-{}", bot_number)),
            DisplayName(format_bot_name(build, bot_number)),
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
    use config::{Elo, Personality};

    #[test]
    fn bot_name_uses_strategy_elo_and_number() {
        let build = BotBuild::new(Elo(2500), Personality::Trapper);
        assert_eq!(format_bot_name(&build, 3), "trapper-2500 #3");
    }

    #[test]
    fn bot_name_covers_roster_personalities() {
        let cases = [
            (Personality::Survivalist, 1000, 1, "survivalist-1000 #1"),
            (Personality::Adaptive, 1500, 2, "adaptive-1500 #2"),
        ];
        for (personality, elo, number, expected) in cases {
            let build = BotBuild::new(Elo(elo), personality);
            assert_eq!(format_bot_name(&build, number), expected);
        }
    }

    #[test]
    fn bot_name_uses_territorialist_label() {
        let build = BotBuild::new(Elo(2000), Personality::Territorialist);
        assert_eq!(format_bot_name(&build, 7), "territorialist-2000 #7");
    }
}
