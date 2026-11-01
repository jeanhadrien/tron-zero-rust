//! Strategy trait and the v1 strategies, plus action selection.
//!
//! The full `score` signature (perception + world + forecaster) is stable from
//! day one; v1 strategies mostly ignore the world and forecaster so algorithms
//! can be iterated on later without touching the scheduler.

use bevy::prelude::Vec2;

use shared::PlayerInput;

use super::config::{BotKnobs, Personality};
use super::forecast::Forecaster;
use super::perceive::{
    Perception, RAY_FRONT, RAY_FRONT_LEFT, RAY_FRONT_RIGHT, RAY_LEFT, RAY_RIGHT,
};
use super::territory::{chamber_stats, voronoi_surplus};
use super::world::{GRID_CELLS, GridCache, RiderSnapshot, WorldSnapshot};

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

impl Mode {
    /// Stable id for the `BotDebug` snapshot.
    pub fn to_u8(self) -> u8 {
        match self {
            Mode::Escape => shared::MODE_ESCAPE,
            Mode::Survival => shared::MODE_SURVIVAL,
            Mode::Attack => shared::MODE_ATTACK,
            Mode::Explore => shared::MODE_EXPLORE,
        }
    }
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
static TERRITORIALIST: Territorialist = Territorialist;
static DUELIST: Duelist = Duelist;

/// Map a personality to its strategy. Only the Roamer placeholder still falls
/// back to `Survivalist` rather than panicking.
pub fn strategy_for(personality: Personality) -> &'static dyn Strategy {
    match personality {
        Personality::Survivalist => &SURVIVALIST,
        Personality::Trapper => &TRAPPER,
        Personality::Adaptive => &ADAPTIVE,
        Personality::Territorialist => &TERRITORIALIST,
        Personality::Duelist => &DUELIST,
        Personality::Roamer => &SURVIVALIST,
    }
}

/// Voronoi territory + chamber control (a1k0n-lite). Scores each action by
/// fork, then measuring the projected map share against the nearest enemy
/// and the farmable chamber value at the projected cell.
///
/// Shape: `Survivalist base + greed*surplus_after + aggression*chamber
/// - caution*threat - TURN_COST on turns`, where the base already carries the
/// `-caution*threat` and turn-cost terms, so they are not subtracted twice.
///
/// Voronoi runs on the *current* grid at projected cells (the 1-step fork only
/// moves ~3 units, far below a grid cell); the projected trail is not
/// re-rasterized, keeping all three forks inside the Tier BFS budget.
pub struct Territorialist;

/// Projected death penalty: worse than any plausible territory swing so a
/// fork that dies in one step is never picked for its chamber value.
const PROJECTED_DEATH: f32 = -1.0;

impl Strategy for Territorialist {
    fn name(&self) -> &'static str {
        "territorialist"
    }

    fn score(
        &self,
        perception: &Perception,
        world: &WorldSnapshot,
        forecast: &dyn Forecaster,
        knobs: &BotKnobs,
    ) -> [f32; 3] {
        let mut scores = Survivalist.score(perception, world, forecast, knobs);
        if perception.rider >= world.riders.len() {
            return scores;
        }
        let enemy_cell =
            nearest_enemy(world, perception.rider).map(|(_, enemy)| GridCache::cell_of(enemy.pos));
        let budget = knobs.bfs_budget.max(1);
        for action in [Action::Hold, Action::Left, Action::Right] {
            let index = action.index();
            let Some((pos, alive)) = forked_step(world, forecast, perception.rider, action) else {
                continue;
            };
            if !alive {
                scores[index] += PROJECTED_DEATH;
                continue;
            }
            let self_cell = GridCache::cell_of(pos);
            let surplus = enemy_cell.map_or(0.0, |enemy| {
                voronoi_surplus(&world.grid, self_cell, enemy, budget) as f32 / GRID_CELLS as f32
            });
            let chamber = chamber_stats(&world.grid, self_cell).value();
            scores[index] += knobs.greed * surplus + knobs.aggression * chamber;
        }
        scores
    }

    fn mode(&self, perception: &Perception) -> Mode {
        if perception.threat > 0.3 {
            Mode::Attack
        } else if perception.freedom < 16 {
            Mode::Survival
        } else {
            Mode::Explore
        }
    }
}

/// Close-combat nose-cutter. At range it rides the survival baseline; once
/// converging on the nearest enemy it scores each of its three moves by a
/// shallow 2-ply search (our 3 x enemy 3, one physics step each) and plays the
/// max over the turn-probability-weighted leaves. Leaves are scored from
/// continuous geometry + rider state only — never the grid, which projected
/// snapshots leave unresolved by design. Rubber courage is deliberate: low
/// rubber costs nothing, only a projected death is penalised.
pub struct Duelist;

/// Duel range before Elo-horizon stretch; matches `DUEL_THREAT` below.
const DUEL_RANGE: f32 = 480.0;
/// `1 - DUEL_RANGE / 600`, the threat proxy `mode` uses for "converging".
const DUEL_THREAT: f32 = 0.2;
/// Cut window ahead of the enemy nose and across it.
const CUT_AHEAD: f32 = 160.0;
const CUT_WIDTH: f32 = 90.0;
/// Max cut bonus (× aggression, ×1.5 head-on); kept below `DUEL_DEATH` so a
/// projected death is never picked for its cut value.
const CUT_GAIN: f32 = 0.9;
/// Projected-death guardrail; dominates any plausible cut swing.
const DUEL_DEATH: f32 = -2.0;

impl Strategy for Duelist {
    fn name(&self) -> &'static str {
        "duelist"
    }

    fn score(
        &self,
        perception: &Perception,
        world: &WorldSnapshot,
        forecast: &dyn Forecaster,
        knobs: &BotKnobs,
    ) -> [f32; 3] {
        let base = Survivalist.score(perception, world, forecast, knobs);
        let (Some(me), Some((foe_index, foe))) = (
            world.riders.get(perception.rider),
            nearest_enemy(world, perception.rider),
        ) else {
            return base;
        };
        if !converging(me.pos, me.dir, foe.pos, foe.dir, knobs) {
            return base;
        }
        // Head-on duels pay extra: crossing the nose there wins the most.
        let gain = if head_on(me.pos, me.dir, foe.pos, foe.dir) {
            1.5
        } else {
            1.0
        };
        // Enemy model from memory: hold with `1 - p`, split `p` over the
        // turns. `p == 0` is exactly assume-straight.
        let turn = perception.opponent_turn_prob.clamp(0.0, 1.0);
        let weights = [1.0 - turn, turn * 0.5, turn * 0.5];
        let mut scores = base;
        for our in [Action::Hold, Action::Left, Action::Right] {
            let mut expected = 0.0;
            for (branch, foe_action) in [Action::Hold, Action::Left, Action::Right]
                .iter()
                .enumerate()
            {
                let projected = project_duel(
                    world,
                    forecast,
                    perception.rider,
                    foe_index,
                    our,
                    *foe_action,
                );
                expected += weights[branch]
                    * duel_leaf(
                        &projected,
                        perception.rider,
                        foe_index,
                        base[our.index()],
                        gain,
                        knobs,
                    );
            }
            scores[our.index()] = expected;
        }
        scores
    }

    fn mode(&self, perception: &Perception) -> Mode {
        if perception.threat > DUEL_THREAT {
            Mode::Attack
        } else {
            default_mode(perception)
        }
    }
}

/// Closing geometry: in range and at least one of us faces the other.
pub(crate) fn converging(
    me_pos: Vec2,
    me_dir: Vec2,
    foe_pos: Vec2,
    foe_dir: Vec2,
    knobs: &BotKnobs,
) -> bool {
    let to_foe = foe_pos - me_pos;
    let dist = to_foe.length();
    if dist < 1.0 {
        return true;
    }
    let range = DUEL_RANGE + knobs.lookahead_ticks as f32 * 8.0;
    if dist > range {
        return false;
    }
    let axis = to_foe / dist;
    me_dir.dot(axis) > 0.0 || foe_dir.dot(-axis) > 0.0
}

/// Both riders committed at each other's noses.
pub(crate) fn head_on(me_pos: Vec2, me_dir: Vec2, foe_pos: Vec2, foe_dir: Vec2) -> bool {
    let to_foe = foe_pos - me_pos;
    let dist = to_foe.length();
    if dist < 1.0 {
        return true;
    }
    let axis = to_foe / dist;
    me_dir.dot(foe_dir) < -0.5 && me_dir.dot(axis) > 0.5 && foe_dir.dot(-axis) > 0.5
}

/// Reward for sitting on the enemy's forward ray, just ahead and close
/// across it: the cut-across-the-nose shape. Zero behind the enemy or wide
/// of the nose.
fn cut_reward(my_pos: Vec2, foe_pos: Vec2, foe_dir: Vec2, knobs: &BotKnobs) -> f32 {
    let rel = my_pos - foe_pos;
    let ahead = rel.dot(foe_dir);
    if ahead < -20.0 {
        return 0.0;
    }
    let lateral = (rel - foe_dir * ahead).length();
    if lateral > CUT_WIDTH {
        return 0.0;
    }
    let window = if ahead <= CUT_AHEAD {
        1.0
    } else {
        (1.0 - (ahead - CUT_AHEAD) / CUT_AHEAD).clamp(0.0, 1.0)
    };
    CUT_GAIN * knobs.aggression * (1.0 - lateral / CUT_WIDTH) * window
}

/// One physics step with our action for us and the given enemy reply
/// (everyone else holds). Reads rider state only; the projected grid stays
/// unresolved and is never consulted.
fn project_duel(
    world: &WorldSnapshot,
    forecast: &dyn Forecaster,
    me_index: usize,
    foe_index: usize,
    our: Action,
    foe: Action,
) -> WorldSnapshot {
    let mut actions = vec![Action::Hold; world.riders.len()];
    if let Some(slot) = actions.get_mut(me_index) {
        *slot = our;
    }
    if let Some(slot) = actions.get_mut(foe_index) {
        *slot = foe;
    }
    forecast.project(world, &actions)
}

/// Leaf value: survival baseline plus the cut, or the death guardrail.
/// `me.rubber` is deliberately ignored — rubber courage, death stops a duel.
fn duel_leaf(
    projected: &WorldSnapshot,
    me_index: usize,
    foe_index: usize,
    base: f32,
    gain: f32,
    knobs: &BotKnobs,
) -> f32 {
    let (Some(me), Some(foe)) = (
        projected.riders.get(me_index),
        projected.riders.get(foe_index),
    ) else {
        return base + DUEL_DEATH;
    };
    if !me.alive {
        return base + DUEL_DEATH;
    }
    if !foe.alive {
        // The cut already landed: bank the kill.
        return base + CUT_GAIN * knobs.aggression * gain;
    }
    base + gain * cut_reward(me.pos, foe.pos, foe.dir, knobs)
}

/// One physics step with `action` for `rider` (everyone else holds).
/// Returns the projected position and aliveness; `None` when the rider index
/// is out of range.
fn forked_step(
    world: &WorldSnapshot,
    forecast: &dyn Forecaster,
    rider: usize,
    action: Action,
) -> Option<(Vec2, bool)> {
    if rider >= world.riders.len() {
        return None;
    }
    let mut actions = vec![Action::Hold; world.riders.len()];
    actions[rider] = action;
    let projected = forecast.project(world, &actions);
    let rider = projected.riders.get(rider)?;
    Some((rider.pos, rider.alive))
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

pub(crate) fn nearest_enemy(
    world: &WorldSnapshot,
    rider: usize,
) -> Option<(usize, &RiderSnapshot)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Vec2;
    use shared::ARENA_WIDTH;
    use std::sync::Arc;

    use super::super::world::GRID_N;

    /// Teleports rider 0 to a per-action cell so scoring sees controlled
    /// projected positions without depending on physics step sizes.
    struct TeleportForecaster {
        targets: [Vec2; 3],
    }

    impl Forecaster for TeleportForecaster {
        fn project(&self, world: &WorldSnapshot, actions: &[Action]) -> WorldSnapshot {
            let mut projected = world.clone();
            if let Some(action) = actions.first()
                && let Some(rider) = projected.riders.first_mut()
            {
                rider.pos = self.targets[action.index()];
            }
            projected
        }
    }

    fn cell_center(gx: usize, gy: usize) -> Vec2 {
        let cell = ARENA_WIDTH / GRID_N as f32;
        let half = ARENA_WIDTH * 0.5;
        Vec2::new(
            (gx as f32 + 0.5) * cell - half,
            (gy as f32 + 0.5) * cell - half,
        )
    }

    fn rider_at(id: u64, pos: Vec2) -> RiderSnapshot {
        RiderSnapshot {
            id,
            pos,
            dir: Vec2::X,
            trail: vec![pos - Vec2::X * 10.0, pos],
            alive: true,
            rubber: 0.0,
            speed_mult: 1.0,
            target_speed_mult: 1.0,
        }
    }

    fn knobs() -> BotKnobs {
        BotKnobs {
            reaction_ticks: 5,
            lookahead_ticks: 4,
            mistake_prob: 0.0,
            greed: 0.9,
            aggression: 0.5,
            caution: 0.1,
            bfs_budget: 1024,
            strategy_mix: 0.5,
        }
    }

    fn perception() -> Perception {
        Perception {
            rays: [f32::INFINITY; super::super::perceive::RAY_COUNT],
            freedom: 800,
            exits: 4,
            threat: 0.0,
            memory_trend: 0.0,
            opponent_turn_prob: 0.0,
            tick: 42,
            rider: 0,
        }
    }

    #[test]
    fn territorialist_prefers_center_control_over_holding_corner() {
        // Self starts in a corner, enemy owns the far corner. Hold stays put,
        // Left teleports to mid-arena (big Voronoi gain), Right to the other
        // corner (symmetric wash). Survivalist holds (no turn cost paid for
        // identical rays); Territorialist pays the turn to take map share.
        let hold = cell_center(3, 3);
        let world = WorldSnapshot::new(
            Arc::new(Vec::new()),
            vec![rider_at(1, hold), rider_at(2, cell_center(28, 28))],
            42,
        );
        let forecast = TeleportForecaster {
            targets: [hold, cell_center(16, 16), cell_center(3, 28)],
        };
        let perception = perception();
        let knobs = knobs();

        let survival_scores = Survivalist.score(&perception, &world, &forecast, &knobs);
        assert_eq!(
            decide(&Survivalist, &perception, &world, &forecast, &knobs),
            Action::Hold
        );

        let control_scores = Territorialist.score(&perception, &world, &forecast, &knobs);
        assert!(
            control_scores[Action::Left.index()] > control_scores[Action::Hold.index()] + 0.05,
            "center share should beat holding the corner: {control_scores:?} vs {survival_scores:?}"
        );
        assert_eq!(
            decide(&Territorialist, &perception, &world, &forecast, &knobs),
            Action::Left
        );
    }

    #[test]
    fn territorialist_penalizes_projected_death() {
        struct Killer;
        impl Forecaster for Killer {
            fn project(&self, world: &WorldSnapshot, _: &[Action]) -> WorldSnapshot {
                let mut projected = world.clone();
                if let Some(rider) = projected.riders.first_mut() {
                    rider.alive = false;
                }
                projected
            }
        }
        let world = WorldSnapshot::new(
            Arc::new(Vec::new()),
            vec![
                rider_at(1, cell_center(10, 10)),
                rider_at(2, cell_center(20, 20)),
            ],
            7,
        );
        let perception = perception();
        let knobs = knobs();
        let base = Survivalist.score(&perception, &world, &Killer, &knobs);
        let scores = Territorialist.score(&perception, &world, &Killer, &knobs);
        for action in [Action::Hold, Action::Left, Action::Right] {
            assert!(
                scores[action.index()] < base[action.index()] - 0.9,
                "dead forks must be penalized: {scores:?} vs base {base:?}"
            );
        }
    }

    #[test]
    fn territorialist_mode_covers_survival_attack_explore() {
        let mut perception = perception();
        assert_eq!(Territorialist.mode(&perception), Mode::Explore);
        perception.threat = 0.5;
        assert_eq!(Territorialist.mode(&perception), Mode::Attack);
        perception.threat = 0.0;
        perception.freedom = 10;
        assert_eq!(Territorialist.mode(&perception), Mode::Survival);
    }

    #[test]
    fn strategy_for_maps_territorialist() {
        assert_eq!(
            strategy_for(Personality::Territorialist).name(),
            "territorialist"
        );
    }

    #[test]
    fn strategy_for_maps_duelist() {
        assert_eq!(strategy_for(Personality::Duelist).name(), "duelist");
    }

    fn rider_facing(id: u64, pos: Vec2, dir: Vec2) -> RiderSnapshot {
        RiderSnapshot {
            id,
            pos,
            dir,
            trail: vec![pos - dir * 10.0, pos],
            alive: true,
            rubber: 0.0,
            speed_mult: 1.0,
            target_speed_mult: 1.0,
        }
    }

    /// Head-on world: us at (-100, 0) heading +X, enemy at (100, 0) heading
    /// -X. Converging with the head-on bonus armed.
    fn head_on_world() -> WorldSnapshot {
        WorldSnapshot::new(
            Arc::new(Vec::new()),
            vec![
                rider_facing(1, Vec2::new(-100.0, 0.0), Vec2::X),
                rider_facing(2, Vec2::new(100.0, 0.0), Vec2::NEG_X),
            ],
            9,
        )
    }

    #[test]
    fn duelist_head_on_prefers_cut_over_hold() {
        // Hold stays wide of the nose (no cut); Left lands on the enemy's
        // forward ray (full cut); Right stays wide too. The turn cost in the
        // survival base is dwarfed by the cut bonus.
        let forecast = TeleportForecaster {
            targets: [
                Vec2::new(-100.0, -150.0),
                Vec2::new(60.0, 0.0),
                Vec2::new(-100.0, 150.0),
            ],
        };
        let world = head_on_world();
        let perception = perception();
        let knobs = knobs();
        let scores = Duelist.score(&perception, &world, &forecast, &knobs);
        assert!(
            scores[Action::Left.index()] > scores[Action::Hold.index()] + 0.2,
            "nose cut should beat holding wide: {scores:?}"
        );
        assert_eq!(
            decide(&Duelist, &perception, &world, &forecast, &knobs),
            Action::Left
        );
    }

    /// Moves both riders per the two actions: ours teleport, the enemy steers
    /// to a different nose offset per reply.
    struct DuelForecaster {
        our_targets: [Vec2; 3],
        foe_targets: [Vec2; 3],
    }

    impl Forecaster for DuelForecaster {
        fn project(&self, world: &WorldSnapshot, actions: &[Action]) -> WorldSnapshot {
            let mut projected = world.clone();
            let our = actions.first().copied().unwrap_or(Action::Hold);
            let foe = actions.get(1).copied().unwrap_or(Action::Hold);
            if let Some(rider) = projected.riders.first_mut() {
                rider.pos = self.our_targets[our.index()];
            }
            if let Some(rider) = projected.riders.get_mut(1) {
                rider.pos = self.foe_targets[foe.index()];
            }
            projected
        }
    }

    #[test]
    fn duelist_turn_rate_zero_assumes_straight() {
        // We sit on the nose when the enemy holds straight; enemy turns flee
        // far away. With turn-rate 0 the turn branches must carry zero weight,
        // so shuffling them cannot move the scores.
        let world = head_on_world();
        let knobs = knobs();
        let near = Vec2::new(100.0, 0.0);
        let ours = [Vec2::new(60.0, 10.0); 3];
        let first = DuelForecaster {
            our_targets: ours,
            foe_targets: [near, Vec2::new(400.0, 400.0), Vec2::new(-400.0, 400.0)],
        };
        let second = DuelForecaster {
            our_targets: ours,
            foe_targets: [near, Vec2::new(400.0, -400.0), Vec2::new(-400.0, -400.0)],
        };
        let mut perception = perception();
        perception.opponent_turn_prob = 0.0;
        let straight_a = Duelist.score(&perception, &world, &first, &knobs);
        let straight_b = Duelist.score(&perception, &world, &second, &knobs);
        assert_eq!(straight_a, straight_b);
        // ...while a turn-heavy prior does listen to those branches.
        perception.opponent_turn_prob = 1.0;
        let turning = Duelist.score(&perception, &world, &second, &knobs);
        assert_ne!(straight_a, turning);
    }

    #[test]
    fn duelist_projected_death_never_picked() {
        struct Killer;
        impl Forecaster for Killer {
            fn project(&self, world: &WorldSnapshot, _: &[Action]) -> WorldSnapshot {
                let mut projected = world.clone();
                if let Some(rider) = projected.riders.first_mut() {
                    rider.alive = false;
                }
                projected
            }
        }
        let world = head_on_world();
        let perception = perception();
        let knobs = knobs();
        let base = Survivalist.score(&perception, &world, &Killer, &knobs);
        let scores = Duelist.score(&perception, &world, &Killer, &knobs);
        for action in [Action::Hold, Action::Left, Action::Right] {
            assert!(
                scores[action.index()] < base[action.index()] - 0.9,
                "dead duels must be penalized: {scores:?} vs base {base:?}"
            );
        }
    }

    #[test]
    fn duelist_mode_is_attack_when_converging() {
        let mut perception = perception();
        perception.threat = 0.5;
        assert_eq!(Duelist.mode(&perception), Mode::Attack);
        perception.threat = 0.0;
        assert_eq!(Duelist.mode(&perception), Mode::Explore);
    }
}
