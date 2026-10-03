//! Strategy trait and the three v1 strategies, plus action selection.
//!
//! The full `score` signature (perception + world + forecaster) is stable from
//! day one; v1 strategies mostly ignore the world and forecaster so algorithms
//! can be iterated on later without touching the scheduler.

use shared::PlayerInput;

use super::config::{BotKnobs, Personality};
use super::forecast::Forecaster;
use super::perceive::{
    Perception, RAY_FRONT, RAY_FRONT_LEFT, RAY_FRONT_RIGHT, RAY_LEFT, RAY_RIGHT,
};
use super::world::{RiderSnapshot, WorldSnapshot};

/// Turn cost keeps a bot from oscillating between equally scored turns.
const TURN_COST: f32 = 0.03;
const TRAP_RANGE: f32 = 320.0;
/// One-step lookahead reward/penalty used by the adaptive mixer.
const LOOKAHEAD_SAFE: f32 = 0.05;
const LOOKAHEAD_DEATH: f32 = -0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Hold,
    Left,
    Right,
}

impl Action {
    pub fn to_input(self) -> PlayerInput {
        match self {
            Action::Hold => PlayerInput::None,
            Action::Left => PlayerInput::TurnLeft,
            Action::Right => PlayerInput::TurnRight,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Action::Hold => 0,
            Action::Left => 1,
            Action::Right => 2,
        }
    }
}

/// JS-parity decision modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Escape,
    Survival,
    Attack,
    Explore,
}

pub trait Strategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn score(
        &self,
        perception: &Perception,
        world: &WorldSnapshot,
        forecast: &dyn Forecaster,
        knobs: &BotKnobs,
    ) -> [f32; 3];
    fn mode(&self, perception: &Perception) -> Mode;
}

/// Maximise freedom/exits and flee from nearby threats.
pub struct Survivalist;

impl Strategy for Survivalist {
    fn name(&self) -> &'static str {
        "survivalist"
    }

    fn score(
        &self,
        perception: &Perception,
        world: &WorldSnapshot,
        _forecast: &dyn Forecaster,
        knobs: &BotKnobs,
    ) -> [f32; 3] {
        // Faster riders need a longer look at open space; speed is a policy
        // input, not a physics buff.
        let speed_scale = world
            .riders
            .get(perception.rider)
            .map(|r| r.speed_mult.max(1.0))
            .unwrap_or(1.0);
        let horizon = super::perceive::SENSE_HORIZON * speed_scale;
        let front = perception.clearance_with(RAY_FRONT, horizon);
        let left = perception.clearance_with(RAY_LEFT, horizon);
        let right = perception.clearance_with(RAY_RIGHT, horizon);
        let front_left = perception.clearance_with(RAY_FRONT_LEFT, horizon);
        let front_right = perception.clearance_with(RAY_FRONT_RIGHT, horizon);

        let open = 0.5 * front + 0.25 * front_left + 0.25 * front_right;
        let turn_left = 0.6 * front_left + 0.4 * left;
        let turn_right = 0.6 * front_right + 0.4 * right;
        let area = 0.30 * perception.freedom_ratio()
            + 0.20 * perception.exits_ratio()
            + 0.10 * perception.memory_trend;
        let danger = perception.threat * knobs.caution;
        [
            open + area - danger,
            turn_left + area - danger - TURN_COST,
            turn_right + area - danger - TURN_COST,
        ]
    }

    fn mode(&self, perception: &Perception) -> Mode {
        default_mode(perception)
    }
}

/// Turn toward the nearest rider when ahead and safe, to cut off territory.
pub struct Trapper;

impl Strategy for Trapper {
    fn name(&self) -> &'static str {
        "trapper"
    }

    fn score(
        &self,
        perception: &Perception,
        world: &WorldSnapshot,
        forecast: &dyn Forecaster,
        knobs: &BotKnobs,
    ) -> [f32; 3] {
        let mut scores = Survivalist.score(perception, world, forecast, knobs);
        let (Some(me), Some((_index, enemy))) = (
            world.riders.get(perception.rider),
            nearest_enemy(world, perception.rider),
        ) else {
            return scores;
        };
        let to_enemy = enemy.pos - me.pos;
        let distance = to_enemy.length();
        let range = TRAP_RANGE + knobs.lookahead_ticks as f32 * 8.0;
        if distance > 1.0 && distance < range {
            let ahead = me.dir.dot(to_enemy / distance);
            if ahead > 0.2 {
                let bonus = knobs.greed * knobs.aggression * (1.0 - distance / range);
                if me.dir.perp_dot(to_enemy) > 0.0 {
                    scores[Action::Left.index()] += bonus;
                } else {
                    scores[Action::Right.index()] += bonus;
                }
            }
        }
        scores
    }

    fn mode(&self, perception: &Perception) -> Mode {
        if perception.threat > 0.25 {
            Mode::Attack
        } else {
            default_mode(perception)
        }
    }
}

/// Blend the other strategies. Concrete regret-matching needs per-bot state;
/// v1 uses a deterministic mix from the `strategy_mix` knob.
pub struct Adaptive;

impl Strategy for Adaptive {
    fn name(&self) -> &'static str {
        "adaptive"
    }

    fn score(
        &self,
        perception: &Perception,
        world: &WorldSnapshot,
        forecast: &dyn Forecaster,
        knobs: &BotKnobs,
    ) -> [f32; 3] {
        let survival = Survivalist.score(perception, world, forecast, knobs);
        let trap = Trapper.score(perception, world, forecast, knobs);
        let mix = knobs.strategy_mix.clamp(0.0, 1.0);
        let mut scores = [
            survival[0] * (1.0 - mix) + trap[0] * mix,
            survival[1] * (1.0 - mix) + trap[1] * mix,
            survival[2] * (1.0 - mix) + trap[2] * mix,
        ];
        // A physics rollout through the shared step: moves that end in a
        // projected death are penalised. This is where the forecaster earns
        // its keep in v1, and where Elo's horizon knob takes effect.
        for action in [Action::Hold, Action::Left, Action::Right] {
            scores[action.index()] += lookahead_safety(perception, world, forecast, action, knobs);
        }
        scores
    }

    fn mode(&self, perception: &Perception) -> Mode {
        if perception.freedom < 24 {
            Mode::Survival
        } else {
            default_mode(perception)
        }
    }
}

static SURVIVALIST: Survivalist = Survivalist;
static TRAPPER: Trapper = Trapper;
static ADAPTIVE: Adaptive = Adaptive;

/// Map a personality to its strategy. Unimplemented personalities fall back to
/// `Survivalist` rather than panicking.
pub fn strategy_for(personality: Personality) -> &'static dyn Strategy {
    match personality {
        Personality::Survivalist => &SURVIVALIST,
        Personality::Trapper => &TRAPPER,
        Personality::Adaptive => &ADAPTIVE,
        Personality::Territorialist | Personality::Duelist | Personality::Roamer => &SURVIVALIST,
    }
}

fn default_mode(perception: &Perception) -> Mode {
    let front = perception.rays[RAY_FRONT];
    if (front.is_finite() && front < super::perceive::SENSE_HORIZON * 0.25)
        || perception.threat > 0.75
    {
        Mode::Escape
    } else if perception.freedom < 16 {
        Mode::Survival
    } else if perception.threat > 0.3 {
        Mode::Attack
    } else {
        Mode::Explore
    }
}

/// Roll this rider forward `lookahead_ticks` with everyone else holding. A
/// projected death is penalised, and dying sooner costs more than later.
fn lookahead_safety(
    perception: &Perception,
    world: &WorldSnapshot,
    forecast: &dyn Forecaster,
    action: Action,
    knobs: &BotKnobs,
) -> f32 {
    if perception.rider >= world.riders.len() {
        return 0.0;
    }
    let steps = knobs.lookahead_ticks.max(1);
    let mut projected = world.clone();
    for step in 0..steps {
        let mut actions = vec![Action::Hold; projected.riders.len()];
        actions[perception.rider] = if step == 0 { action } else { Action::Hold };
        projected = forecast.project(&projected, &actions);
        let survived = projected
            .riders
            .get(perception.rider)
            .is_some_and(|rider| rider.alive);
        if !survived {
            let horizon_left = 1.0 - step as f32 / steps as f32;
            return LOOKAHEAD_DEATH * horizon_left;
        }
    }
    LOOKAHEAD_SAFE
}

fn nearest_enemy(world: &WorldSnapshot, rider: usize) -> Option<(usize, &RiderSnapshot)> {
    let me = world.riders.get(rider)?;
    world
        .riders
        .iter()
        .enumerate()
        .filter(|(index, other)| *index != rider && other.alive)
        .min_by(|(_, a), (_, b)| me.pos.distance(a.pos).total_cmp(&me.pos.distance(b.pos)))
}

/// Argmax over blended scores, with a deterministic mistake roll.
pub fn decide(
    strategy: &dyn Strategy,
    perception: &Perception,
    world: &WorldSnapshot,
    forecast: &dyn Forecaster,
    knobs: &BotKnobs,
) -> Action {
    let scores = strategy.score(perception, world, forecast, knobs);
    let mut best = Action::Hold;
    let mut best_score = f32::NEG_INFINITY;
    for action in [Action::Hold, Action::Left, Action::Right] {
        if scores[action.index()] > best_score {
            best_score = scores[action.index()];
            best = action;
        }
    }
    let mut rng = BotRng::seed((perception.tick as u64) ^ ((perception.rider as u64) << 32));
    if knobs.mistake_prob > 0.0 && rng.next_f32() < knobs.mistake_prob {
        return match rng.next_u32() % 3 {
            0 => Action::Hold,
            1 => Action::Left,
            _ => Action::Right,
        };
    }
    best
}

/// Tiny deterministic xorshift, seeded from tick + rider. Keeps bot mistakes
/// reproducible without pulling the shared sim into any RNG state.
struct BotRng(u64);

impl BotRng {
    fn seed(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}
