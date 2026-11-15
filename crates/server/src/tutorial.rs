//! The tutorial director: plays steps one attempt at a time for one human, on
//! the real sim. It replaces the match ([`crate::game`]): nobody respawns, the
//! director places riders itself, and the replicated `MatchStatus` is a
//! constant so the client and the protocol stay as they are.
//!
//! An attempt: retire everyone, hold a countdown, place the player in a new
//! life (stale turns die), then judge every tick after the deaths are
//! resolved: pass or fail, show the result for a moment, then the next step or
//! the same step again. The status goes out through a shared [`TutorialStatus`]
//! (the hosted server's handle), never on the wire.

use std::sync::{Arc, Mutex};

use bevy::math::Rect;
use bevy::prelude::*;
use lightyear::prelude::*;
use scene::Scene;
use shared::spawn::{SpawnField, placed_at};
use shared::{IsAlive, LifeGeneration, MatchPhase, MatchStatus, Player, Position, TICK_HZ};

use crate::systems::{Human, InitialSpawn};

/// Hold before the player is placed: time to read the prompt.
pub const COUNTDOWN_TICKS: u32 = 2 * TICK_HZ as u32;
/// How long a pass or a fail is shown before the next attempt.
pub const RESULT_TICKS: u32 = TICK_HZ as u32;
/// The scene rider the human plays.
pub const PLAYER: &str = "player";

/// One lesson: a scene, what to do in it, and in how long.
#[derive(Clone, Debug)]
pub struct Step {
    pub prompt: String,
    pub scene: Scene,
    pub goal: Goal,
    /// Ticks the player has, counted from placement.
    pub timeout: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Goal {
    /// The player's head inside `area`.
    Reach { area: Rect },
}

/// Why an attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    Death,
    Timeout,
}

impl Fail {
    pub fn label(self) -> &'static str {
        match self {
            Self::Death => "You crashed.",
            Self::Timeout => "Out of time.",
        }
    }
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
    out: StatusCell,
    /// The replicated status entity, spawned on first use.
    status: Option<Entity>,
}

impl Tutorial {
    pub fn new(steps: Vec<Step>, out: StatusCell) -> Self {
        Self {
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
        let length = match self.phase {
            Phase::Countdown => COUNTDOWN_TICKS,
            Phase::Playing => step.timeout,
            Phase::Passed | Phase::Failed => RESULT_TICKS,
            Phase::Waiting | Phase::Done => 0,
        };
        let Goal::Reach { area } = step.goal;
        TutorialStatus {
            step: self.step,
            steps: self.steps.len(),
            attempt: self.attempt,
            phase: self.phase,
            ticks_left: length.saturating_sub(self.ticks),
            prompt: step.prompt.clone(),
            goal_area: Some(area),
            fail: self.fail,
        }
    }
}

/// The first tutorial, until steps are read from a file: reach a square that
/// needs a turn.
pub fn builtin_steps() -> Result<Vec<Step>, String> {
    let scene = Scene::parse("[[rider]]\nname = \"player\"\nat = [-600, -600]\nheading = \"+x\"")?;
    Ok(vec![Step {
        prompt: "Reach the marked square. Turn with your left and right keys.".into(),
        scene,
        goal: Goal::Reach {
            area: Rect::new(300.0, 200.0, 600.0, 500.0),
        },
        timeout: 20 * TICK_HZ as u32,
    }])
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

/// Retire every rider and hold the countdown of a new attempt.
fn start_attempt(world: &mut World) {
    let alive: Vec<Entity> = world
        .query_filtered::<(Entity, &IsAlive), With<Player>>()
        .iter(world)
        .filter(|(_, alive)| alive.0)
        .map(|(entity, _)| entity)
        .collect();
    crate::game::retire(world, &alive);
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

/// Put the player at the step's start in a new life, so turns sent for an
/// earlier attempt are stale.
fn place_player(world: &mut World, entity: Entity) {
    let tutorial = world.resource::<Tutorial>();
    let Some(rider) = tutorial
        .current()
        .scene
        .riders
        .iter()
        .find(|rider| rider.name == PLAYER)
        .cloned()
    else {
        warn!("Tutorial: the step has no {PLAYER:?} rider");
        return;
    };
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
    let head = world.get::<Position>(entity).map(|p| p.0);
    let mut tutorial = world.resource_mut::<Tutorial>();
    let Goal::Reach { area } = tutorial.current().goal;
    let outcome = if alive && head.is_some_and(|head| area.contains(head)) {
        Ok(())
    } else if !alive {
        Err(Fail::Death)
    } else if tutorial.ticks >= tutorial.current().timeout {
        Err(Fail::Timeout)
    } else {
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
        world.insert_resource(Tutorial::new(builtin_steps().unwrap(), out.clone()));
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
            1 + COUNTDOWN_TICKS + builtin_steps().unwrap()[0].timeout,
        );
        assert_eq!(status(&out).fail, Some(Fail::Timeout));
    }

    #[test]
    fn the_match_lets_nobody_respawn_in_a_tutorial() {
        let (mut world, _, _) = setup();
        let riders = crate::game::Riders::of(&mut world);
        assert!(!crate::game::may_spawn(&world, true, &riders));
        assert!(!crate::game::may_spawn(&world, false, &riders));
    }
}
