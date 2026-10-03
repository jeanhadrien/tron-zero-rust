//! Bot difficulty curve and per-bot policy configuration.
//!
//! Difficulty is expressed only through *policy* knobs (reaction, horizon,
//! mistake rate, weighting). Physics is never touched.

use serde::{Deserialize, Serialize};

/// Rating anchors. `Elo::knobs` interpolates policy between them.
pub const ELO_ANCHORS: [u32; 5] = [500, 1000, 1500, 2500, 4000];

/// Policy curve sampled at `ELO_ANCHORS`; smoothstep-eased between anchors.
const REACTION_TICKS: [f32; 5] = [24.0, 16.0, 10.0, 5.0, 2.0];
const LOOKAHEAD_TICKS: [f32; 5] = [2.0, 4.0, 6.0, 10.0, 16.0];
const MISTAKE_PROB: [f32; 5] = [0.30, 0.15, 0.07, 0.02, 0.003];
const GREED: [f32; 5] = [0.10, 0.30, 0.50, 0.75, 0.95];
const AGGRESSION: [f32; 5] = [0.05, 0.20, 0.40, 0.70, 0.95];
const CAUTION: [f32; 5] = [0.95, 0.80, 0.60, 0.35, 0.15];
const SENSOR_RAYS: [f32; 5] = [3.0, 5.0, 7.0, 7.0, 7.0];
const BFS_BUDGET: [f32; 5] = [64.0, 128.0, 256.0, 512.0, 800.0];
const STRATEGY_MIX: [f32; 5] = [0.0, 0.25, 0.5, 0.8, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elo(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Personality {
    Survivalist,
    Trapper,
    Adaptive,
    Territorialist,
    /// Named placeholder; not implemented yet.
    Duelist,
    /// Named placeholder; not implemented yet.
    Roamer,
}

/// Policy-only knobs. Never physics values: bots drive ordinary inputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotKnobs {
    pub reaction_ticks: u32,
    pub lookahead_ticks: u32,
    pub mistake_prob: f32,
    pub greed: f32,
    pub aggression: f32,
    pub caution: f32,
    pub sensor_rays: u32,
    pub bfs_budget: u32,
    pub strategy_mix: f32,
}

impl BotKnobs {
    fn clamp(&mut self) {
        self.reaction_ticks = self.reaction_ticks.max(1);
        self.lookahead_ticks = self.lookahead_ticks.max(1);
        self.sensor_rays = self.sensor_rays.clamp(1, 7);
        self.bfs_budget = self.bfs_budget.max(1);
        self.mistake_prob = self.mistake_prob.clamp(0.0, 1.0);
        self.greed = self.greed.clamp(0.0, 1.0);
        self.aggression = self.aggression.clamp(0.0, 1.0);
        self.caution = self.caution.clamp(0.0, 1.0);
        self.strategy_mix = self.strategy_mix.clamp(0.0, 1.0);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotBuild {
    pub elo: Elo,
    pub personality: Personality,
    pub knobs: BotKnobs,
}

impl BotBuild {
    pub fn new(elo: Elo, personality: Personality) -> Self {
        Self {
            elo,
            personality,
            knobs: elo.knobs(personality),
        }
    }
}

impl Elo {
    /// Interpolate the policy curve at this rating, then apply personality
    /// biases. Static per bot: no adaptive difficulty drift.
    pub fn knobs(self, personality: Personality) -> BotKnobs {
        let elo = self.0;
        let mut knobs = BotKnobs {
            reaction_ticks: interp(&REACTION_TICKS, elo).round() as u32,
            lookahead_ticks: interp(&LOOKAHEAD_TICKS, elo).round() as u32,
            mistake_prob: interp(&MISTAKE_PROB, elo),
            greed: interp(&GREED, elo),
            aggression: interp(&AGGRESSION, elo),
            caution: interp(&CAUTION, elo),
            sensor_rays: interp(&SENSOR_RAYS, elo).round() as u32,
            bfs_budget: interp(&BFS_BUDGET, elo).round() as u32,
            strategy_mix: interp(&STRATEGY_MIX, elo),
        };
        personality.apply(&mut knobs);
        knobs.clamp();
        knobs
    }
}

impl Personality {
    fn apply(self, knobs: &mut BotKnobs) {
        match self {
            Personality::Survivalist => {
                knobs.caution += 0.1;
                knobs.aggression -= 0.15;
                knobs.greed -= 0.1;
            }
            Personality::Trapper => {
                knobs.aggression += 0.2;
                knobs.greed += 0.1;
                knobs.caution -= 0.1;
            }
            // Control-leaning: wants map share and chambers, accepts risk.
            Personality::Territorialist => {
                knobs.greed += 0.15;
                knobs.aggression += 0.1;
                knobs.caution -= 0.05;
            }
            // Adaptive deliberately keeps the Elo-derived `strategy_mix`, and
            // remaining placeholders share the base curve until implemented.
            Personality::Adaptive | Personality::Duelist | Personality::Roamer => {}
        }
    }
}

/// Smoothstep easing keeps the curve "logistic-ish" without a table of match
/// arms. Clamps to the end anchors outside the sampled range.
fn interp(values: &[f32; 5], elo: u32) -> f32 {
    if elo <= ELO_ANCHORS[0] {
        return values[0];
    }
    if elo >= ELO_ANCHORS[4] {
        return values[4];
    }
    for i in 0..4 {
        let low = ELO_ANCHORS[i];
        let high = ELO_ANCHORS[i + 1];
        if elo <= high {
            let t = (elo - low) as f32 / (high - low) as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            return values[i] + (values[i + 1] - values[i]) * eased;
        }
    }
    values[4]
}

/// Default arena roster: graded difficulty and mixed personalities so a single
/// weak bot is not farmed.
pub fn default_builds() -> Vec<BotBuild> {
    vec![
        BotBuild::new(Elo(1000), Personality::Survivalist),
        BotBuild::new(Elo(1500), Personality::Adaptive),
        BotBuild::new(Elo(2000), Personality::Territorialist),
        BotBuild::new(Elo(2500), Personality::Trapper),
    ]
}
