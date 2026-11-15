//! The tutorial director: plays steps one attempt at a time for one human, on
//! the real sim. It replaces the match ([`crate::game`]): nobody respawns, the
//! director places riders itself, and the replicated `MatchStatus` is a
//! constant so the client and the protocol stay as they are.
//!
//! An attempt: retire everyone, set the step's walls and seat its other
//! riders, hold a countdown, apply the scene (every rider placed where the
//! scene says, the player in a new life so stale turns die, the bot and script
//! clock back to 0), then judge every tick after the deaths are resolved: pass
//! or fail, show the result for a moment, then the next step or the same step
//! again. The status goes out through a shared [`TutorialStatus`] (the hosted
//! server's handle), never on the wire.
//!
//! Scene riders play as in any runner of a step: the same placements
//! (`scene::Scene::place`, seed `scene::step::SEED`), scripts and bot
//! seeds from placement tick 0, and the same `scene::step::Judge` fed with
//! scene names.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bevy::math::Rect;
use bevy::prelude::*;
use lightyear::prelude::*;
use scene::Action;
use scene::step::{Area, Death, Judge, PLAYER, RiderState, Step};
use shared::spawn::{SpawnField, placed_at};
use shared::{
    ActionState, Arena, ArenaSize, DeathCause, DeathLog, DeathRecord, IsAlive, IsColliding,
    LifeGeneration, MatchPhase, MatchStatus, Player, PlayerInput, Position, Rubber, SpeedMult,
    TICK_HZ, WallSegments,
};

pub use scene::step::Fail;

use crate::bot::{BotBrain, DriveTick};
use crate::systems::{Human, InitialSpawn};

/// Hold before the player is placed: time to read the prompt.
pub const COUNTDOWN_TICKS: u32 = 2 * TICK_HZ as u32;
/// How long a pass or a fail is shown before the next attempt.
pub const RESULT_TICKS: u32 = TICK_HZ as u32;

/// Parse a step file and check it is playable here: on top of
/// `scene::step::parse`, every step's riders can be placed on its arena.
pub fn load(text: &str) -> Result<Vec<Step>, String> {
    let steps = scene::step::parse(text)?;
    for (i, step) in steps.iter().enumerate() {
        let mut world = World::new();
        let size = ArenaSize::default();
        world.spawn((Arena, size, WallSegments(step_walls(step, size))));
        let mut field = SpawnField::walls(&mut world).ok_or("no arena")?;
        step.scene
            .place(&mut field, scene::step::SEED)
            .map_err(|e| format!("step {}: {e}", i + 1))?;
    }
    Ok(steps)
}

/// A rider the scene seats besides the player, by its index in the step's
/// scene. Lives for one attempt.
#[derive(Component)]
struct SceneRider(usize);

/// A scripted rider's moves, by tick from placement ([`DriveTick`]).
#[derive(Component)]
struct Script(BTreeMap<u32, Action>);

/// This tick's deaths, copied for the judge before `resolve_deaths` drains
/// the log.
#[derive(Resource, Default)]
struct TickDeaths(Vec<DeathRecord>);

/// The tutorial's systems around the shared simulation. Call after
/// `add_core` and `bot::add_drive`.
pub(crate) fn add_systems(app: &mut App) {
    app.init_resource::<TickDeaths>();
    app.add_systems(
        FixedUpdate,
        (
            // Same tick as the bots, read before `bot_brain_input` moves it on.
            script_input
                .after(crate::systems::handle_respawns)
                .before(crate::bot::bot_brain_input)
                .before(shared::simulate_players),
            // The sim fills the log; `resolve_deaths` drains it.
            copy_deaths
                .after(shared::simulate_players)
                .before(crate::elimination::resolve_deaths),
        ),
    );
}

/// Each scripted rider's input for this tick: its move due now, else hold
/// (inputs are one-shot, as in the lab).
fn script_input(tick: Res<DriveTick>, mut riders: Query<(&Script, &mut ActionState<PlayerInput>)>) {
    for (script, mut input) in &mut riders {
        input.0 = match script.0.get(&tick.0) {
            Some(Action::Left) => PlayerInput::TurnLeft,
            Some(Action::Right) => PlayerInput::TurnRight,
            _ => PlayerInput::None,
        };
    }
}

fn copy_deaths(log: Res<DeathLog>, mut out: ResMut<TickDeaths>) {
    out.0.clone_from(&log.0);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// No player yet.
    #[default]
    Waiting,
    Countdown,
    Playing,
    Passed,
    Failed,
    /// Every step passed.
    Done,
}

/// What the client shows of the tutorial.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TutorialStatus {
    /// Index of the current step, and how many there are.
    pub step: usize,
    pub steps: usize,
    /// Attempts started so far (1 for the first).
    pub attempt: u32,
    pub phase: Phase,
    /// Ticks left in the countdown, the attempt or the result.
    pub ticks_left: u32,
    pub prompt: String,
    /// The area the goal marks, if any.
    pub goal_area: Option<Rect>,
    pub fail: Option<Fail>,
}

/// The shared status a hosted tutorial writes and its handle reads.
pub type StatusCell = Arc<Mutex<Option<TutorialStatus>>>;

#[derive(Resource)]
pub struct Tutorial {
    steps: Vec<Step>,
    step: usize,
    attempt: u32,
    phase: Phase,
    /// Ticks spent in the current phase.
    ticks: u32,
    fail: Option<Fail>,
    /// Decides the attempt while `Playing`; a new one at each placement.
    judge: Judge,
    out: StatusCell,
    /// The replicated status entity, spawned on first use.
    status: Option<Entity>,
}

impl Tutorial {
    /// `steps` must not be empty (`load` checks it).
    pub fn new(steps: Vec<Step>, out: StatusCell) -> Self {
        Self {
            judge: Judge::new(&steps[0]),
            steps,
            step: 0,
            attempt: 0,
            phase: Phase::Waiting,
            ticks: 0,
            fail: None,
            out,
            status: None,
        }
    }

    fn current(&self) -> &Step {
        &self.steps[self.step]
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.ticks = 0;
    }

    fn view(&self) -> TutorialStatus {
        let step = self.current();
        let ticks_left = match self.phase {
            Phase::Playing => self.judge.ticks_left(),
            Phase::Countdown => COUNTDOWN_TICKS.saturating_sub(self.ticks),
            Phase::Passed | Phase::Failed => RESULT_TICKS.saturating_sub(self.ticks),
            Phase::Waiting | Phase::Done => 0,
        };
        TutorialStatus {
            step: self.step,
            steps: self.steps.len(),
            attempt: self.attempt,
            phase: self.phase,
            ticks_left,
            prompt: step.prompt.clone(),
            goal_area: step.goal.area().map(rect),
            fail: self.fail.clone(),
        }
    }
}

fn rect(area: Area) -> Rect {
    Rect::from_corners(area.min, area.max)
}

/// Run the tutorial one tick. Runs where `advance_match` runs on a match
/// server: after the tick's simulation and death resolution.
pub fn advance_tutorial(world: &mut World) {
    let human = world
        .query_filtered::<(Entity, &IsAlive), With<Human>>()
        .iter(world)
        .next()
        .map(|(entity, alive)| (entity, alive.0));
    let (phase, ticks) = {
        let mut tutorial = world.resource_mut::<Tutorial>();
        tutorial.ticks += 1;
        (tutorial.phase, tutorial.ticks)
    };
    match (human, phase) {
        (None, Phase::Waiting) => {}
        // The player left: wait for the next one, from the same step.
        (None, _) => {
            clear_scene(world);
            world.resource_mut::<Tutorial>().enter(Phase::Waiting);
        }
        (Some(_), Phase::Waiting) => start_attempt(world),
        (Some((entity, _)), Phase::Countdown) if ticks >= COUNTDOWN_TICKS => {
            apply_scene(world, entity);
            world.resource_mut::<Tutorial>().enter(Phase::Playing);
        }
        (Some((entity, alive)), Phase::Playing) => judge(world, entity, alive),
        (Some(_), Phase::Failed) if ticks >= RESULT_TICKS => start_attempt(world),
        (Some(_), Phase::Passed) if ticks >= RESULT_TICKS => {
            let mut tutorial = world.resource_mut::<Tutorial>();
            if tutorial.step + 1 < tutorial.steps.len() {
                tutorial.step += 1;
                start_attempt(world);
            } else {
                info!("Tutorial: every step passed");
                tutorial.enter(Phase::Done);
            }
        }
        _ => {}
    }
    publish(world);
}

/// Retire every rider, set the step's walls, seat its other riders (dead
/// until placed) and hold the countdown of a new attempt. Seating them now
/// gives their replicated entities the countdown to reach the client.
fn start_attempt(world: &mut World) {
    clear_scene(world);
    set_walls(world);
    let step = world.resource::<Tutorial>().current().clone();
    for (index, rider) in step.scene.riders.iter().enumerate() {
        if index == step.player {
            continue;
        }
        let seat = crate::bot::seat(format!("scene-{index}"), rider.name.clone(), index);
        let mut entity = world.spawn((seat, SceneRider(index)));
        // A bot gets its brain when placed; any other rider rides its script.
        if rider.bot.is_none() {
            entity.insert(Script(rider.script.clone()));
        }
    }
    let mut tutorial = world.resource_mut::<Tutorial>();
    tutorial.attempt += 1;
    tutorial.fail = None;
    tutorial.enter(Phase::Countdown);
    info!(
        step = tutorial.step,
        attempt = tutorial.attempt,
        "Tutorial: attempt starts"
    );
}

/// The arena boundary plus the current step's walls. Called only while every
/// rider is retired, and the countdown that follows is longer than a client
/// rollback: no simulated tick, here or re-simulated on a client, sees a
/// rider and a wall change together. Unchanged walls are not replicated
/// again.
fn set_walls(world: &mut World) {
    let tutorial = world.resource::<Tutorial>();
    let step = tutorial.current().clone();
    let mut arena = world.query_filtered::<(&ArenaSize, &mut WallSegments), With<Arena>>();
    let Ok((size, mut walls)) = arena.single_mut(world) else {
        warn!("Tutorial: no arena to set the walls on");
        return;
    };
    let next = step_walls(&step, *size);
    walls.set_if_neq(WallSegments(next));
}

/// The arena boundary of `size` plus the step's own walls.
fn step_walls(step: &Step, size: ArenaSize) -> Vec<[f32; 4]> {
    let mut walls = shared::arena_walls(size.width, size.height);
    walls.extend(step.scene.walls.iter().copied());
    walls
}

/// Retire every living rider and remove the riders the scene seated.
fn clear_scene(world: &mut World) {
    retire_all(world);
    let seated: Vec<Entity> = world
        .query_filtered::<Entity, With<SceneRider>>()
        .iter(world)
        .collect();
    for entity in seated {
        world.despawn(entity);
    }
}

fn retire_all(world: &mut World) {
    let alive: Vec<Entity> = world
        .query_filtered::<(Entity, &IsAlive), With<Player>>()
        .iter(world)
        .filter(|(_, alive)| alive.0)
        .map(|(entity, _)| entity)
        .collect();
    crate::game::retire(world, &alive);
}

/// Place every rider of the step where the scene says: the player (`human`)
/// in a new life, so turns sent for an earlier attempt are stale; the seated
/// riders, bots with a fresh brain. Restart the bot and script clock and the
/// judge. Runs at the end of a tick: the next tick is placement tick 0.
fn apply_scene(world: &mut World, human: Entity) {
    let step = world.resource::<Tutorial>().current().clone();
    world.resource_mut::<Tutorial>().judge = Judge::new(&step);
    world.resource_mut::<DriveTick>().0 = 0;
    let placements = SpawnField::walls(world)
        .ok_or_else(|| "no arena".to_owned())
        .and_then(|mut field| step.scene.place(&mut field, scene::step::SEED));
    let placements = match placements {
        Ok(placements) => placements,
        // `load` placed every step already: only a broken arena gets here.
        Err(e) => {
            warn!("Tutorial: cannot place the scene: {e}");
            return;
        }
    };
    let seated: Vec<(Entity, usize)> = world
        .query::<(Entity, &SceneRider)>()
        .iter(world)
        .map(|(entity, rider)| (entity, rider.0))
        .collect();
    for (index, placement) in placements.into_iter().enumerate() {
        let entity = if index == step.player {
            Some(human)
        } else {
            seated.iter().find(|(_, i)| *i == index).map(|(e, _)| *e)
        };
        let Some(entity) = entity else {
            warn!(index, "Tutorial: a scene rider was not seated");
            continue;
        };
        let mut rider = world.entity_mut(entity);
        if index == step.player {
            rider.remove::<InitialSpawn>();
            if let Some(life) = rider.get::<LifeGeneration>().and_then(|g| g.next()) {
                rider.insert(life);
            }
        }
        rider.insert(placed_at(&placement.start));
        if let Some(brain) = placement.brain {
            rider.insert(BotBrain { brain });
        }
    }
}

/// The scene name of `entity`: the player for `human`, a seated rider's own
/// name, `None` for anything else.
fn scene_name<'a>(world: &World, step: &'a Step, human: Entity, entity: Entity) -> Option<&'a str> {
    if entity == human {
        return Some(PLAYER);
    }
    let index = world.get::<SceneRider>(entity)?.0;
    step.scene.riders.get(index).map(|r| r.name.as_str())
}

/// Pass or fail the attempt on this tick's state.
fn judge(world: &mut World, entity: Entity, alive: bool) {
    let player = world.entity(entity);
    let state = RiderState {
        alive,
        position: player.get::<Position>().map_or(Vec2::ZERO, |p| p.0),
        speed: player.get::<SpeedMult>().map_or(0.0, |s| s.0),
        colliding: player.get::<IsColliding>().is_some_and(|c| c.0),
        rubber: player.get::<Rubber>().map_or(0.0, |r| r.0),
    };
    let outcome = world.resource_scope(|world, mut tutorial: Mut<Tutorial>| {
        let tutorial = &mut *tutorial;
        let step = &tutorial.steps[tutorial.step];
        let deaths: Vec<Death> = world
            .resource::<TickDeaths>()
            .0
            .iter()
            .filter_map(|death| {
                let victim = scene_name(world, step, entity, death.victim)?;
                let killer = match death.cause {
                    DeathCause::Trail(owner) => scene_name(world, step, entity, owner),
                    DeathCause::Wall | DeathCause::OwnTrail => None,
                };
                Some(Death { victim, killer })
            })
            .collect();
        tutorial.judge.observe(&state, &deaths)
    });
    let Some(outcome) = outcome else {
        return;
    };
    let mut tutorial = world.resource_mut::<Tutorial>();
    match outcome {
        Ok(()) => {
            info!(step = tutorial.step, "Tutorial: step passed");
            tutorial.enter(Phase::Passed);
        }
        Err(fail) => {
            info!(step = tutorial.step, ?fail, "Tutorial: attempt failed");
            tutorial.fail = Some(fail);
            tutorial.enter(Phase::Failed);
        }
    }
    // Nobody rides while the result is shown.
    retire_all(world);
}

/// Write the status for the handle, and keep the replicated `MatchStatus` a
/// constant "playing, no respawn".
fn publish(world: &mut World) {
    let tutorial = world.resource::<Tutorial>();
    let view = tutorial.view();
    *tutorial
        .out
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(view);
    if tutorial.status.is_none() {
        let status = MatchStatus {
            rounds: 1,
            round: 1,
            phase: MatchPhase::Playing,
            secs_left: None,
            winner: None,
            respawn_now: false,
        };
        let entity = world
            .spawn((status, Replicate::to_clients(NetworkTarget::All)))
            .id();
        world.resource_mut::<Tutorial>().status = Some(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIDERS: &str = include_str!("../../scene/fixtures/riders.toml");

    fn setup() -> (World, Entity, StatusCell) {
        let mut world = World::new();
        let out = StatusCell::default();
        world.insert_resource(Tutorial::new(
            load(scene::step::BUILTIN).unwrap(),
            out.clone(),
        ));
        world.init_resource::<DriveTick>();
        world.init_resource::<TickDeaths>();
        world.spawn((
            shared::Arena,
            shared::ArenaSize::default(),
            shared::WallSegments(shared::arena_walls(
                shared::ARENA_WIDTH,
                shared::ARENA_HEIGHT,
            )),
        ));
        let human = world
            .spawn((
                Player,
                Human,
                InitialSpawn,
                LifeGeneration(7),
                IsAlive(false),
            ))
            .id();
        (world, human, out)
    }

    fn status(out: &StatusCell) -> TutorialStatus {
        out.lock().unwrap().clone().unwrap()
    }

    fn run(world: &mut World, ticks: u32) {
        for _ in 0..ticks {
            advance_tutorial(world);
        }
    }

    #[test]
    fn an_attempt_places_the_player_in_a_new_life_after_the_countdown() {
        let (mut world, human, out) = setup();
        run(&mut world, 1);
        assert_eq!(status(&out).phase, Phase::Countdown);
        assert_eq!(status(&out).attempt, 1);
        run(&mut world, COUNTDOWN_TICKS);
        assert_eq!(status(&out).phase, Phase::Playing);
        assert!(world.get::<IsAlive>(human).unwrap().0);
        assert!(world.get::<InitialSpawn>(human).is_none());
        assert_eq!(world.get::<LifeGeneration>(human).unwrap().0, 8);
        assert_eq!(
            world.get::<Position>(human).unwrap().0,
            Vec2::new(-600.0, -600.0)
        );
        // The match status says: playing, nobody respawns.
        let match_status = world.query::<&MatchStatus>().single(&world).unwrap();
        assert!(!match_status.respawn_now);
    }

    #[test]
    fn a_death_fails_and_restarts_the_same_step_in_another_life() {
        let (mut world, human, out) = setup();
        run(&mut world, 1 + COUNTDOWN_TICKS);
        world.entity_mut(human).insert(IsAlive(false));
        run(&mut world, 1);
        assert_eq!(status(&out).phase, Phase::Failed);
        assert_eq!(status(&out).fail, Some(Fail::Death));
        run(&mut world, RESULT_TICKS + COUNTDOWN_TICKS);
        let now = status(&out);
        assert_eq!((now.phase, now.attempt, now.step), (Phase::Playing, 2, 0));
        assert_eq!(now.fail, None);
        assert_eq!(world.get::<LifeGeneration>(human).unwrap().0, 9);
    }

    #[test]
    fn reaching_the_area_passes_and_idling_times_out() {
        let (mut world, human, out) = setup();
        run(&mut world, 1 + COUNTDOWN_TICKS);
        world
            .entity_mut(human)
            .insert(Position(Vec2::new(450.0, 300.0)));
        run(&mut world, 1);
        assert_eq!(status(&out).phase, Phase::Passed);
        assert!(!world.get::<IsAlive>(human).unwrap().0, "retired");
        run(&mut world, RESULT_TICKS);
        assert_eq!(status(&out).phase, Phase::Done);

        let (mut world, _, out) = setup();
        run(
            &mut world,
            1 + COUNTDOWN_TICKS + load(scene::step::BUILTIN).unwrap()[0].timeout,
        );
        assert_eq!(status(&out).fail, Some(Fail::Timeout));
    }

    #[test]
    fn the_shipped_tutorial_and_steps_with_riders_load_here() {
        assert!(!load(scene::step::BUILTIN).unwrap().is_empty());
        assert_eq!(load(RIDERS).unwrap().len(), 4);
    }

    /// The server's tick as `add_core` orders it, without the network: scripts
    /// and bots, the sim, the deaths for the judge, the director.
    fn tick_schedule() -> Schedule {
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (
                script_input,
                crate::bot::bot_brain_input,
                shared::simulate_players,
                copy_deaths,
                // `resolve_deaths` drains the log on a server.
                |mut log: ResMut<DeathLog>| log.0.clear(),
                advance_tutorial,
            )
                .chain(),
        );
        schedule
    }

    /// The world of `setup` playing step `index` of the rider fixtures, run up
    /// to the tick that places the scene. The next tick is placement tick 0.
    fn placed(index: usize) -> (World, Schedule, Entity, StatusCell) {
        let (mut world, human, out) = setup();
        let mut tutorial = Tutorial::new(load(RIDERS).unwrap(), out.clone());
        tutorial.step = index;
        world.insert_resource(tutorial);
        world.insert_resource(shared::SimulationRole::Server);
        world.init_resource::<DeathLog>();
        world.init_resource::<crate::bot::SchedulerConfig>();
        world.init_resource::<crate::bot::BotDebugConfig>();
        let mut schedule = tick_schedule();
        while world.resource::<Tutorial>().phase != Phase::Playing {
            schedule.run(&mut world);
        }
        (world, schedule, human, out)
    }

    fn rider(world: &mut World, name: &str) -> Entity {
        world
            .query::<(Entity, &shared::DisplayName)>()
            .iter(world)
            .find(|(_, n)| n.0 == name)
            .map(|(e, _)| e)
            .unwrap()
    }

    #[test]
    fn a_scripted_rider_follows_its_script_from_the_placement_tick() {
        let (mut world, mut schedule, _, _) = placed(0);
        let guide = rider(&mut world, "guide");
        assert_eq!(
            world.get::<Position>(guide).unwrap().0,
            Vec2::new(0.0, 600.0)
        );
        let mut turned = None;
        for tick in 0..120 {
            schedule.run(&mut world);
            let heading = world.get::<shared::Direction>(guide).unwrap().0;
            if heading != Vec2::X && turned.is_none() {
                turned = Some((tick, heading));
            }
        }
        // As in the lab (`crates/lab/tests/tutorial.rs`): tick 60, left of +x.
        assert_eq!(turned, Some((60, Vec2::Y)));
    }

    #[test]
    fn deaths_reach_the_judge_by_scene_name() {
        let (mut world, mut schedule, _, out) = placed(1);
        let mut tutorial = world.resource_mut::<Tutorial>();
        assert_eq!(tutorial.attempt, 1);
        tutorial.steps.truncate(2);
        while status(&out).phase == Phase::Playing {
            schedule.run(&mut world);
        }
        assert_eq!(status(&out).phase, Phase::Passed, "{:?}", status(&out).fail);
        let alive = world
            .query::<&IsAlive>()
            .iter(&world)
            .filter(|a| a.0)
            .count();
        assert_eq!(alive, 0, "nobody rides while the result shows");

        let (mut world, mut schedule, _, out) = placed(2);
        while status(&out).phase == Phase::Playing {
            schedule.run(&mut world);
        }
        // Re-seating on the next attempt despawns replicated entities: the
        // nettest `a_tutorial_scene_rider_reaches_the_client_and_is_reseated`.
        assert_eq!(status(&out).fail, Some(Fail::RiderDead("victim".into())));
    }

    #[test]
    fn a_scene_bot_is_placed_with_a_brain_and_rides() {
        let (mut world, mut schedule, _, _) = placed(3);
        let bot = rider(&mut world, "rival");
        assert!(world.get::<BotBrain>(bot).is_some());
        assert!(world.get::<Script>(bot).is_none());
        for _ in 0..60 {
            schedule.run(&mut world);
        }
        assert!(world.get::<IsAlive>(bot).unwrap().0);
    }

    #[test]
    fn an_attempt_sets_the_steps_walls_on_the_arena() {
        let text = scene::step::BUILTIN
            .replace("[step.scene]", "[step.scene]\nwalls = [[0, -100, 0, 100]]");
        let (mut world, _, _) = setup();
        world.insert_resource(Tutorial::new(load(&text).unwrap(), StatusCell::default()));
        run(&mut world, 1);
        let walls = world.query::<&WallSegments>().single(&world).unwrap();
        assert_eq!(walls.0.len(), 5);
        assert_eq!(walls.0[4], [0.0, -100.0, 0.0, 100.0]);
    }
}
