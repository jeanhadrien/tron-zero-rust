//! Server-only bot management and the look → plan → emit AI loop.
//!
//! The brain itself lives in the `bots` crate; this module owns the ECS side:
//! the roster, spawning, respawns and the `BotDebug` writer.
//! `bot_brain_input` stays in the FixedUpdate chain before `simulate_players`
//! and writes every alive bot's `ActionState` each tick. A bot is one entity
//! for the life of the server: it dies, waits, and is placed again on a safe
//! spot by `respawn_bots`, like a human.

pub(crate) mod debug;

use bevy::prelude::*;
use bots::Brain;
use bots::config::BotBuild;
use bots::drive::{Seat, drive};
use bots::world::WorldSnapshotBuilder;
use lightyear::prelude::*;
use shared::spawn::{SpawnField, placed};
use shared::{ActionState, DisplayName, IsAlive, Player, PlayerColor, PlayerInput};

pub use bots::schedule::SchedulerConfig;
pub use debug::BotDebugConfig;

/// Ticks a dead bot waits before it is placed again.
const RESPAWN_DELAY_TICKS: u32 = 240;

const BOT_COLORS: [u32; 8] = [
    0xFF4444, 0x44FF44, 0x4488FF, 0xFFCC00, 0xFF44FF, 0x44FFFF, 0xFF8800, 0x8844FF,
];

/// Per-bot AI state. A respawn recreates it from the same `BotBuild`.
#[derive(Component)]
pub struct BotBrain {
    pub brain: Brain,
}

/// Ticks left before a dead bot is placed again.
#[derive(Component)]
struct RespawnIn(u32);

/// The builds the server seats, one bot each.
#[derive(Resource)]
pub struct BotRoster(pub Vec<BotBuild>);

/// Spawn one bot per build of the roster, dead and due at once: `respawn_bots`
/// places them on the first fixed tick, like any later respawn.
pub fn setup_bots(mut commands: Commands, roster: Res<BotRoster>) {
    for (slot, build) in roster.0.iter().enumerate() {
        let entity = commands
            .spawn((
                Player,
                BotBrain {
                    brain: Brain::new(build.clone(), rand::random(), slot),
                },
                shared::PlayerId(format!("bot-{}", slot + 1)),
                DisplayName(build.name.clone()),
                PlayerColor(BOT_COLORS[slot % BOT_COLORS.len()]),
                IsAlive(false),
                RespawnIn(1),
                Replicate::to_clients(NetworkTarget::All),
                InterpolationTarget::to_clients(NetworkTarget::All),
            ))
            .id();
        // Inserts a default `shared::BotDebug` only when `TRON_BOT_DEBUG` is
        // truthy; the writer overwrites it on the next planning round.
        if debug::enabled_from_env() {
            commands.entity(entity).insert(shared::BotDebug::default());
        }
    }
}

/// Look → plan → emit for every alive bot, through `bots::drive`: the
/// decisions are the bots', this system adds the logs and the debug overlay.
pub fn bot_brain_input(
    mut commands: Commands,
    debug_cfg: Res<BotDebugConfig>,
    scheduler: Res<SchedulerConfig>,
    mut ticks: Local<u32>,
    builder: WorldSnapshotBuilder,
    mut brains: Query<(Entity, &mut BotBrain, &IsAlive), With<Player>>,
    mut inputs: Query<&mut ActionState<PlayerInput>, With<BotBrain>>,
) {
    let tick = *ticks;
    *ticks = ticks.wrapping_add(1);

    let mut bots: Vec<_> = brains
        .iter_mut()
        .map(|(entity, brain, alive)| (entity, alive.0, brain))
        .collect();
    let mut seats: Vec<Seat> = bots
        .iter_mut()
        .map(|(entity, alive, brain)| Seat {
            entity: *entity,
            alive: *alive,
            brain: &mut brain.brain,
        })
        .collect();
    let actions = drive(&mut seats, &builder, tick, &scheduler, None, |mut look| {
        let decision = look.act();
        if let Some(report) = &decision.report {
            debug!(
                bot = %look.brain.build.name,
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
                    look.world,
                    look.index,
                    decision.action,
                    &look.brain.build.knobs,
                );
                debug::insert_if_enabled(&mut commands, &debug_cfg, look.entity, snapshot);
            }
        }
        decision
    });
    for (seat, action) in seats.iter().zip(actions) {
        if let Ok(mut input) = inputs.get_mut(seat.entity) {
            input.0 = action.to_input();
        }
    }
}

/// Place dead bots again: `RESPAWN_DELAY_TICKS` after the death, the same
/// entity gets a safe spawn (the search humans use) and a fresh brain. In
/// rounds mode the round flow places the bots instead. Exclusive: a spawn must
/// see the riders placed a moment ago, humans' included.
pub fn respawn_bots(world: &mut World) {
    let rounds_run = world
        .get_resource::<crate::rounds::MatchState>()
        .is_some_and(|state| state.is_running());
    let bots: Vec<(Entity, bool, Option<u32>)> = world
        .query_filtered::<(Entity, &IsAlive, Option<&RespawnIn>), (With<BotBrain>, With<Player>)>()
        .iter(world)
        .map(|(entity, alive, timer)| (entity, alive.0, timer.map(|t| t.0)))
        .collect();
    for (entity, alive, timer) in bots {
        if alive || rounds_run {
            if timer.is_some() {
                world.entity_mut(entity).remove::<RespawnIn>();
            }
            continue;
        }
        let left = timer.unwrap_or(RESPAWN_DELAY_TICKS).saturating_sub(1);
        if left > 0 {
            world.entity_mut(entity).insert(RespawnIn(left));
            continue;
        }
        let spawn =
            SpawnField::with_living(world).and_then(|mut field| field.place(rand::random()));
        let mut bot = world.entity_mut(entity);
        let Some(spawn) = spawn else {
            warn!(?entity, "Bot respawn: no safe spawn, trying again later");
            bot.insert(RespawnIn(RESPAWN_DELAY_TICKS));
            continue;
        };
        bot.insert(placed(&spawn));
        bot.remove::<RespawnIn>();
        if let Some(mut bot) = bot.get_mut::<BotBrain>() {
            bot.brain = bot.brain.reborn(rand::random());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arena and one dead bot due at once.
    fn arena_with_bot() -> (World, Entity) {
        let mut world = World::new();
        world.spawn((
            shared::Arena,
            shared::ArenaSize::default(),
            shared::WallSegments(shared::arena_walls(
                shared::ARENA_WIDTH,
                shared::ARENA_HEIGHT,
            )),
        ));
        let build = bots::roster::builtin().unwrap().remove(0);
        let bot = world
            .spawn((
                Player,
                BotBrain {
                    brain: Brain::new(build, 1, 0),
                },
                IsAlive(false),
                RespawnIn(1),
            ))
            .id();
        (world, bot)
    }

    #[test]
    fn a_dead_bot_is_placed_again_as_the_same_entity_on_a_safe_spot() {
        let (mut world, bot) = arena_with_bot();
        let rival = world
            .spawn((
                Player,
                IsAlive(true),
                shared::Position(Vec2::new(200.0, 0.0)),
                shared::Trail::new(Vec2::new(200.0, 0.0)),
            ))
            .id();
        respawn_bots(&mut world);
        assert!(world.get::<IsAlive>(bot).unwrap().0);
        assert!(world.get::<RespawnIn>(bot).is_none());
        let at = world.get::<shared::Position>(bot).unwrap().0;
        let heading = world.get::<shared::Direction>(bot).unwrap().0;
        let rival_at = world.get::<shared::Position>(rival).unwrap().0;
        let spawn = shared::spawn::SafeSpawn {
            position: at,
            direction: heading,
        };
        let walls: Vec<_> = shared::arena_walls(shared::ARENA_WIDTH, shared::ARENA_HEIGHT)
            .iter()
            .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
            .collect();
        assert!(shared::spawn::spawn_is_safe(
            spawn,
            shared::ArenaSize::default(),
            &walls,
            &[rival_at]
        ));
        assert_eq!(
            world
                .query_filtered::<Entity, With<BotBrain>>()
                .iter(&world)
                .count(),
            1
        );
    }

    #[test]
    fn a_bot_waits_after_its_death_and_is_never_replaced_while_alive() {
        let (mut world, bot) = arena_with_bot();
        respawn_bots(&mut world);
        // Alive for far longer than the old auto-cycle: same entity, same place.
        let at = world.get::<shared::Position>(bot).unwrap().0;
        for _ in 0..4000 {
            respawn_bots(&mut world);
        }
        assert_eq!(world.get::<shared::Position>(bot).unwrap().0, at);
        // It dies: it stays dead for the delay, then comes back.
        world.entity_mut(bot).insert(IsAlive(false));
        for _ in 0..RESPAWN_DELAY_TICKS - 1 {
            respawn_bots(&mut world);
            assert!(!world.get::<IsAlive>(bot).unwrap().0);
        }
        respawn_bots(&mut world);
        assert!(world.get::<IsAlive>(bot).unwrap().0);
        assert_eq!(
            world
                .query_filtered::<Entity, With<BotBrain>>()
                .iter(&world)
                .count(),
            1
        );
    }
}
