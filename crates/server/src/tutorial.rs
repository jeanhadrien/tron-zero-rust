//! The tutorial director: plays steps one attempt at a time for one human, on
//! the real sim. It replaces the match ([`crate::game`]): nobody respawns, the
//! director places riders itself, and the replicated `MatchStatus` is a
//! constant so the client and the protocol stay as they are.
//!
//! An attempt: retire everyone, set the step's walls, hold a countdown, place
//! the player in a new life (stale turns die), then judge every tick after the deaths are
//! resolved: pass or fail, show the result for a moment, then the next step or
//! the same step again. The status goes out through a shared [`TutorialStatus`]
//! (the hosted server's handle), never on the wire.

use std::sync::{Arc, Mutex};

use bevy::math::Rect;
use bevy::prelude::*;
use lightyear::prelude::*;
use scene::step::{Area, Judge, RiderState, Step};
use shared::spawn::{SpawnField, placed_at};
use shared::{
    Arena, ArenaSize, IsAlive, IsColliding, LifeGeneration, MatchPhase, MatchStatus, Player,
    Position, Rubber, SpeedMult, TICK_HZ, WallSegments,
};

pub use scene::step::Fail;

use crate::systems::{Human, InitialSpawn};

/// Hold before the player is placed: time to read the prompt.
pub const COUNTDOWN_TICKS: u32 = 2 * TICK_HZ as u32;
/// How long a pass or a fail is shown before the next attempt.
pub const RESULT_TICKS: u32 = TICK_HZ as u32;

/// Parse a step file and check it is playable here. On top of
/// `scene::step::parse`: no riders but the player, until the server seats
/// scripted riders and bots, so a step never plays differently here than in
/// the lab.
pub fn load(text: &str) -> Result<Vec<Step>, String> {
    let steps = scene::step::parse(text)?;
    for (i, step) in steps.iter().enumerate() {
        if step.scene.riders.len() != 1 {
            return Err(format!(
                "step {}: riders other than the player are not hosted yet",
                i + 1
            ));
        }
    }
    Ok(steps)
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
        (None, _) => world.resource_mut::<Tutorial>().enter(Phase::Waiting),
        (Some(_), Phase::Waiting) => start_attempt(world),
        (Some((entity, _)), Phase::Countdown) if ticks >= COUNTDOWN_TICKS => {
            place_player(world, entity);
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

/// Retire every rider, set the step's walls and hold the countdown of a new
/// attempt.
fn start_attempt(world: &mut World) {
    let alive: Vec<Entity> = world
        .query_filtered::<(Entity, &IsAlive), With<Player>>()
        .iter(world)
        .filter(|(_, alive)| alive.0)
        .map(|(entity, _)| entity)
        .collect();
    crate::game::retire(world, &alive);
    set_walls(world);
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
    let extra = tutorial.current().scene.walls.clone();
    let mut arena = world.query_filtered::<(&ArenaSize, &mut WallSegments), With<Arena>>();
    let Ok((size, mut walls)) = arena.single_mut(world) else {
        warn!("Tutorial: no arena to set the walls on");
        return;
    };
    let mut next = shared::arena_walls(size.width, size.height);
    next.extend(extra);
    walls.set_if_neq(WallSegments(next));
}

/// Put the player at the step's start in a new life, so turns sent for an
/// earlier attempt are stale, and start judging.
fn place_player(world: &mut World, entity: Entity) {
    let mut tutorial = world.resource_mut::<Tutorial>();
    tutorial.judge = Judge::new(tutorial.current());
    let rider = tutorial.current().player().clone();
    let seed = u64::from(tutorial.attempt);
    let spawn = match rider.at {
        Some(at) => Some(at),
        None => SpawnField::walls(world).and_then(|mut field| field.place(seed)),
    };
    let Some(spawn) = spawn else {
        warn!("Tutorial: no safe spawn for the player");
        return;
    };
    let mut player = world.entity_mut(entity);
    player.remove::<InitialSpawn>();
    if let Some(life) = player.get::<LifeGeneration>().and_then(|g| g.next()) {
        player.insert(life);
    }
    player.insert(placed_at(&rider.start(spawn)));
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
    let mut tutorial = world.resource_mut::<Tutorial>();
    // Only the player rides yet (`load`), so no other death can matter.
    let Some(outcome) = tutorial.judge.observe(&state, &[]) else {
        return;
    };
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
    crate::game::retire(world, &[entity]);
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

    fn setup() -> (World, Entity, StatusCell) {
        let mut world = World::new();
        let out = StatusCell::default();
        world.insert_resource(Tutorial::new(
            load(scene::step::BUILTIN).unwrap(),
            out.clone(),
        ));
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
    fn the_shipped_tutorial_loads_here_and_unhosted_scenes_do_not() {
        assert!(!load(scene::step::BUILTIN).unwrap().is_empty());
        let crowded = format!(
            "{}\n[[step.scene.rider]]\nname = \"other\"\n",
            scene::step::BUILTIN
        );
        assert!(load(&crowded).unwrap_err().contains("not hosted yet"));
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
