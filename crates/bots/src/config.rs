//! Per-bot configuration: a name, a personality and a budget.
//!
//! Difficulty is the budget ([`BotKnobs`]: how fast the bot reacts, how far it
//! looks, how many options it weighs, how wrongly it judges). Personality is
//! the direction ([`Weights`]: what the bot wants). Physics is never touched.
//! How good a build *is* is measured by the matchmaking rating, never declared
//! here. [`Difficulty`] is only a convenience for authoring a coherent budget.

use serde::{Deserialize, Serialize};

use crate::objective::Weights;

/// Anchors of the authoring curve. `Difficulty::knobs` interpolates between them.
const DIFFICULTY_ANCHORS: [u32; 5] = [500, 1000, 1500, 2500, 4000];

/// Budget curve sampled at `DIFFICULTY_ANCHORS`; smoothstep-eased between anchors.
const REACTION_TICKS: [f32; 5] = [20.0, 10.0, 7.0, 4.0, 2.0];
const HORIZON_TICKS: [f32; 5] = [14.0, 32.0, 40.0, 52.0, 64.0];
const VARIETY: [f32; 5] = [1.0, 1.6, 2.0, 2.7, 3.0];
const NOISE: [f32; 5] = [0.70, 0.40, 0.28, 0.10, 0.0];

/// Authoring preset: a point on the difficulty curve that expands into a
/// coherent budget. Not stored on a bot, not rated, never shown; once
/// expanded, the knobs are the bot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Difficulty(pub u32);

/// What a bot wants; picks its [`Weights`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Personality {
    Survivalist,
    Trapper,
    Adaptive,
    Territorialist,
    Duelist,
    /// Plain base weights.
    Roamer,
    /// Adaptive without any will to fight (no pressure, control or
    /// engagement): the reference opponent of the fight measurements.
    Passive,
}

/// The budget. Never physics values: bots drive ordinary inputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotKnobs {
    /// Ticks between two plannings: reaction time.
    pub reaction_ticks: u32,
    /// Ticks ahead every candidate is ridden, judged and verified.
    pub horizon_ticks: u32,
    /// How many kinds of maneuver are weighed: 1 turn now, 2 adds delayed
    /// turns and U-turns, 3 adds dodges and tight U-turns.
    pub variety: u32,
    /// Misjudgment of distances and room, `0` = none. A bot with noise reads a
    /// wall as farther, or a pocket as larger, than it is; it never acts at
    /// random.
    pub noise: f32,
}

impl BotKnobs {
    /// Stable 32-bit FNV-1a hash of the personality, its weights and every
    /// knob. Two builds with the same hash are the same bot, wherever they run.
    fn fingerprint(&self, personality: Personality) -> u32 {
        let knobs = [
            self.reaction_ticks,
            self.horizon_ticks,
            self.variety,
            self.noise.to_bits(),
        ];
        let weights = personality.weights();
        let weights = weights
            .values()
            .into_iter()
            .chain([weights.risk])
            .map(f32::to_bits)
            .collect::<Vec<u32>>();
        personality
            .label()
            .bytes()
            .chain(knobs.iter().flat_map(|word| word.to_le_bytes()))
            .chain(weights.iter().flat_map(|word| word.to_le_bytes()))
            .fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            })
    }

    fn clamp(&mut self) {
        self.reaction_ticks = self.reaction_ticks.max(1);
        self.horizon_ticks = self.horizon_ticks.max(1);
        self.variety = self.variety.clamp(1, 3);
        self.noise = self.noise.clamp(0.0, 1.0);
    }
}

/// What defines a bot: a name, a personality and its budget. The name is both
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

    /// A build whose budget comes from a point on the authoring curve.
    pub fn from_difficulty(
        name: impl Into<String>,
        personality: Personality,
        difficulty: Difficulty,
    ) -> Self {
        Self::new(name, personality, difficulty.knobs())
    }

    /// The objective weights of this build.
    pub fn weights(&self) -> Weights {
        self.personality.weights()
    }

    /// Stable identity of this build in the rating pool:
    /// `bot:<name>:<fingerprint of personality, weights and knobs>`. Every
    /// instance of the build shares one rating, which starts like any
    /// newcomer's and is measured. Renaming or retuning a build makes it a new
    /// rated bot.
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
    pub horizon_ticks: Option<u32>,
    pub variety: Option<u32>,
    pub noise: Option<f32>,
}

impl KnobOverrides {
    /// Apply the set overrides to `knobs`, then re-clamp.
    pub fn apply(&self, knobs: &mut BotKnobs) {
        if let Some(v) = self.reaction_ticks {
            knobs.reaction_ticks = v;
        }
        if let Some(v) = self.horizon_ticks {
            knobs.horizon_ticks = v;
        }
        if let Some(v) = self.variety {
            knobs.variety = v;
        }
        if let Some(v) = self.noise {
            knobs.noise = v;
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
            Personality::Passive => "passive",
        }
    }

    /// The objective of this personality. Every one but `Passive` wants to
    /// fight; they differ in how much room and margin they want, how freely
    /// they turn, and how they fight: the survivalist keeps its distance, the
    /// territorialist grabs the arena, the duelist hunts and takes risks, the
    /// trapper shuts opponents in.
    pub fn weights(self) -> Weights {
        let base = Weights::BASE;
        match self {
            Personality::Survivalist => Weights {
                space: 1.3,
                clear: 1.1,
                turn: 0.2,
                pressure: 0.5,
                control: 0.2,
                engage: 0.25,
                grind: 2.0,
                rubber: 0.9,
                risk: 0.08,
                ..base
            },
            Personality::Territorialist => Weights {
                space: 1.2,
                fill: 0.7,
                pressure: 1.0,
                control: 0.8,
                engage: 0.5,
                grind: 3.0,
                rubber: 0.6,
                risk: 0.2,
                ..base
            },
            Personality::Duelist => Weights {
                clear: 0.6,
                turn: 0.1,
                pressure: 2.5,
                control: 0.3,
                engage: 0.8,
                grind: 6.0,
                rubber: 0.15,
                risk: 0.4,
                ..base
            },
            Personality::Trapper => Weights {
                turn: 0.12,
                commit: 0.3,
                pressure: 2.0,
                control: 0.5,
                engage: 0.4,
                grind: 4.5,
                rubber: 0.4,
                risk: 0.25,
                ..base
            },
            Personality::Adaptive | Personality::Roamer => base,
            Personality::Passive => base.passive(),
        }
    }
}

impl Difficulty {
    /// Interpolate the budget curve at this point. Static per bot: no
    /// adaptive difficulty drift.
    pub fn knobs(self) -> BotKnobs {
        let level = self.0;
        let mut knobs = BotKnobs {
            reaction_ticks: interp(&REACTION_TICKS, level).round() as u32,
            horizon_ticks: interp(&HORIZON_TICKS, level).round() as u32,
            variety: interp(&VARIETY, level).round() as u32,
            noise: interp(&NOISE, level),
        };
        knobs.clamp();
        knobs
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
        let mut knobs = Difficulty(1500).knobs();
        let before = knobs.clone();
        KnobOverrides {
            reaction_ticks: Some(3),
            ..KnobOverrides::default()
        }
        .apply(&mut knobs);
        assert_eq!(knobs.reaction_ticks, 3);
        assert_eq!(knobs.horizon_ticks, before.horizon_ticks);
    }

    /// Every budget knob improves (never worsens) with difficulty, so a
    /// higher difficulty is never a worse bot by construction.
    #[test]
    fn the_budget_curve_is_monotonic() {
        let levels: Vec<BotKnobs> = (500..=4000)
            .step_by(100)
            .map(|level| Difficulty(level).knobs())
            .collect();
        for pair in levels.windows(2) {
            assert!(pair[1].reaction_ticks <= pair[0].reaction_ticks);
            assert!(pair[1].horizon_ticks >= pair[0].horizon_ticks);
            assert!(pair[1].variety >= pair[0].variety);
            assert!(pair[1].noise <= pair[0].noise);
        }
    }
}
