//! Per-bot policy configuration.
//!
//! A bot is a name, a personality and a set of named policy knobs (reaction,
//! horizon, mistake rate, weighting). Physics is never touched. The knobs are
//! the bot's whole definition; how good it *is* is measured by the matchmaking
//! rating, never declared here. [`Difficulty`] is only a convenience for
//! authoring a coherent set of knobs.

use serde::{Deserialize, Serialize};

/// Anchors of the authoring curve. `Difficulty::knobs` interpolates between them.
const DIFFICULTY_ANCHORS: [u32; 5] = [500, 1000, 1500, 2500, 4000];

/// Policy curve sampled at `DIFFICULTY_ANCHORS`; smoothstep-eased between anchors.
const REACTION_TICKS: [f32; 5] = [24.0, 16.0, 10.0, 5.0, 2.0];
const LOOKAHEAD_TICKS: [f32; 5] = [2.0, 4.0, 6.0, 10.0, 16.0];
const MISTAKE_PROB: [f32; 5] = [0.30, 0.15, 0.07, 0.02, 0.003];
const GREED: [f32; 5] = [0.10, 0.30, 0.50, 0.75, 0.95];
const AGGRESSION: [f32; 5] = [0.05, 0.20, 0.40, 0.70, 0.95];
const CAUTION: [f32; 5] = [0.95, 0.80, 0.60, 0.35, 0.15];
const BFS_BUDGET: [f32; 5] = [64.0, 128.0, 256.0, 512.0, 800.0];
const STRATEGY_MIX: [f32; 5] = [0.0, 0.25, 0.5, 0.8, 1.0];

/// Authoring preset: a point on the difficulty curve that expands into a
/// coherent set of knobs. Not stored on a bot, not rated, never shown; once
/// expanded, the knobs are the bot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Difficulty(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Personality {
    Survivalist,
    Trapper,
    Adaptive,
    Territorialist,
    /// Close-combat nose-cutter; seeks head-on duels and cuts across bows.
    Duelist,
    /// Named placeholder; not implemented yet.
    Roamer,
}

/// Policy-only knobs. Never physics values: bots drive ordinary inputs.
/// Sensing is always the full 8-ray fan, so there is no ray-count knob.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotKnobs {
    pub reaction_ticks: u32,
    pub lookahead_ticks: u32,
    pub mistake_prob: f32,
    pub greed: f32,
    pub aggression: f32,
    pub caution: f32,
    pub bfs_budget: u32,
    pub strategy_mix: f32,
}

impl BotKnobs {
    /// Stable 32-bit FNV-1a hash of the personality and every knob. Two builds
    /// with the same personality and knobs are the same bot, wherever they run.
    fn fingerprint(&self, personality: Personality) -> u32 {
        let words = [
            self.reaction_ticks,
            self.lookahead_ticks,
            self.mistake_prob.to_bits(),
            self.greed.to_bits(),
            self.aggression.to_bits(),
            self.caution.to_bits(),
            self.bfs_budget,
            self.strategy_mix.to_bits(),
        ];
        personality
            .label()
            .bytes()
            .chain(words.iter().flat_map(|word| word.to_le_bytes()))
            .fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            })
    }

    fn clamp(&mut self) {
        self.reaction_ticks = self.reaction_ticks.max(1);
        self.lookahead_ticks = self.lookahead_ticks.max(1);
        self.bfs_budget = self.bfs_budget.max(1);
        self.mistake_prob = self.mistake_prob.clamp(0.0, 1.0);
        self.greed = self.greed.clamp(0.0, 1.0);
        self.aggression = self.aggression.clamp(0.0, 1.0);
        self.caution = self.caution.clamp(0.0, 1.0);
        self.strategy_mix = self.strategy_mix.clamp(0.0, 1.0);
    }
}

/// What defines a bot: a name, a personality and its knobs. The name is both
/// its display name in game and its label on the leaderboard.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotBuild {
    pub name: String,
    pub personality: Personality,
    pub knobs: BotKnobs,
}

impl BotBuild {
    pub fn new(name: impl Into<String>, personality: Personality, knobs: BotKnobs) -> Self {
        Self {
            name: name.into(),
            personality,
            knobs,
        }
    }

    /// A build whose knobs come from a point on the authoring curve.
    pub fn from_difficulty(
        name: impl Into<String>,
        personality: Personality,
        difficulty: Difficulty,
    ) -> Self {
        Self::new(name, personality, difficulty.knobs(personality))
    }

    /// Stable identity of this build in the rating pool:
    /// `bot:<name>:<fingerprint of personality and knobs>`. Every instance of
    /// the build shares one rating, which starts like any newcomer's and is
    /// measured. Renaming or retuning a build makes it a new rated bot.
    pub fn rating_id(&self) -> String {
        format!(
            "bot:{}:{:08x}",
            self.name.replace(':', "-"),
            self.knobs.fingerprint(self.personality)
        )
    }
}

/// Optional per-knob overrides on top of a preset (scenario files).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnobOverrides {
    pub reaction_ticks: Option<u32>,
    pub lookahead_ticks: Option<u32>,
    pub mistake_prob: Option<f32>,
    pub greed: Option<f32>,
    pub aggression: Option<f32>,
    pub caution: Option<f32>,
    pub bfs_budget: Option<u32>,
    pub strategy_mix: Option<f32>,
}

impl KnobOverrides {
    /// Apply the set overrides to `knobs`, then re-clamp.
    pub fn apply(&self, knobs: &mut BotKnobs) {
        if let Some(v) = self.reaction_ticks {
            knobs.reaction_ticks = v;
        }
        if let Some(v) = self.lookahead_ticks {
            knobs.lookahead_ticks = v;
        }
        if let Some(v) = self.mistake_prob {
            knobs.mistake_prob = v;
        }
        if let Some(v) = self.greed {
            knobs.greed = v;
        }
        if let Some(v) = self.aggression {
            knobs.aggression = v;
        }
        if let Some(v) = self.caution {
            knobs.caution = v;
        }
        if let Some(v) = self.bfs_budget {
            knobs.bfs_budget = v;
        }
        if let Some(v) = self.strategy_mix {
            knobs.strategy_mix = v;
        }
        knobs.clamp();
    }
}

impl Personality {
    /// Lowercase name used in rating ids and logs.
    pub fn label(self) -> &'static str {
        match self {
            Personality::Survivalist => "survivalist",
            Personality::Trapper => "trapper",
            Personality::Adaptive => "adaptive",
            Personality::Territorialist => "territorialist",
            Personality::Duelist => "duelist",
            Personality::Roamer => "roamer",
        }
    }
}

impl Difficulty {
    /// Interpolate the policy curve at this point, then apply personality
    /// biases. Static per bot: no adaptive difficulty drift.
    pub fn knobs(self, personality: Personality) -> BotKnobs {
        let level = self.0;
        let mut knobs = BotKnobs {
            reaction_ticks: interp(&REACTION_TICKS, level).round() as u32,
            lookahead_ticks: interp(&LOOKAHEAD_TICKS, level).round() as u32,
            mistake_prob: interp(&MISTAKE_PROB, level),
            greed: interp(&GREED, level),
            aggression: interp(&AGGRESSION, level),
            caution: interp(&CAUTION, level),
            bfs_budget: interp(&BFS_BUDGET, level).round() as u32,
            strategy_mix: interp(&STRATEGY_MIX, level),
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
            // Close-combat lean: seeks contact, tolerates rubber burns.
            Personality::Duelist => {
                knobs.aggression += 0.25;
                knobs.caution -= 0.15;
                knobs.greed += 0.05;
            }
            // Adaptive deliberately keeps the curve's `strategy_mix`, and
            // remaining placeholders share the base curve until implemented.
            Personality::Adaptive | Personality::Roamer => {}
        }
    }
}

/// Smoothstep easing keeps the curve "logistic-ish" without a table of match
/// arms. Clamps to the end anchors outside the sampled range.
fn interp(values: &[f32; 5], level: u32) -> f32 {
    if level <= DIFFICULTY_ANCHORS[0] {
        return values[0];
    }
    if level >= DIFFICULTY_ANCHORS[4] {
        return values[4];
    }
    for i in 0..4 {
        let low = DIFFICULTY_ANCHORS[i];
        let high = DIFFICULTY_ANCHORS[i + 1];
        if level <= high {
            let t = (level - low) as f32 / (high - low) as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            return values[i] + (values[i + 1] - values[i]) * eased;
        }
    }
    values[4]
}

/// Default arena roster: graded difficulty and mixed personalities so a single
/// weak bot is not farmed.
pub fn default_builds() -> Vec<BotBuild> {
    [
        ("Bastion", Personality::Survivalist, 1000),
        ("Chameleon", Personality::Adaptive, 1500),
        ("Warden", Personality::Territorialist, 2000),
        ("Rapier", Personality::Duelist, 2250),
        ("Sentinelle", Personality::Trapper, 2500),
    ]
    .into_iter()
    .map(|(name, personality, level)| {
        BotBuild::from_difficulty(name, personality, Difficulty(level))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_id_follows_name_personality_and_knobs() {
        let build = BotBuild::from_difficulty("Sentinelle", Personality::Trapper, Difficulty(2500));
        let same = BotBuild::from_difficulty("Sentinelle", Personality::Trapper, Difficulty(2500));
        assert_eq!(build.rating_id(), same.rating_id());
        assert!(build.rating_id().starts_with("bot:Sentinelle:"));

        let other_personality =
            BotBuild::from_difficulty("Sentinelle", Personality::Duelist, Difficulty(2500));
        assert_ne!(build.rating_id(), other_personality.rating_id());

        let mut tweaked = build.clone();
        tweaked.knobs.reaction_ticks += 1;
        assert_ne!(build.rating_id(), tweaked.rating_id());
    }

    #[test]
    fn roster_builds_have_distinct_names_and_identities() {
        let builds = default_builds();
        let ids: std::collections::HashSet<_> = builds.iter().map(BotBuild::rating_id).collect();
        let names: std::collections::HashSet<_> = builds.iter().map(|b| &b.name).collect();
        assert_eq!(ids.len(), builds.len());
        assert_eq!(names.len(), builds.len());
    }

    #[test]
    fn overrides_replace_only_the_set_knobs() {
        let mut knobs = Difficulty(1500).knobs(Personality::Adaptive);
        let before = knobs.clone();
        KnobOverrides {
            reaction_ticks: Some(3),
            ..KnobOverrides::default()
        }
        .apply(&mut knobs);
        assert_eq!(knobs.reaction_ticks, 3);
        assert_eq!(knobs.lookahead_ticks, before.lookahead_ticks);
    }
}
