//! Scenario files: a small TOML description of an arena, riders and their
//! per-tick scripts. The sim is deterministic, so exploring a situation means
//! editing the file and re-running from tick 0.
//!
//! ```toml
//! until = 600                      # ticks to simulate (120 per second)
//! arena = [2400, 2400]             # optional, defaults to the game arena
//! walls = [[-200, 100, 200, 100]]  # optional extra segments x1 y1 x2 y2
//!
//! [[rider]]
//! name = "a"
//! at = [-1000, 0]
//! heading = "+x"                   # +x | -x | +y | -y (y points up)
//! speed = 1.0                      # optional starting target speed
//! rubber = 120                     # optional starting rubber
//! script = ["40 left", "100 right", "300 respawn"]
//! ```
//!
//! Bot arenas: a rider with `bot` is driven by the server's bot brain instead
//! of a script. Leaving out `at`/`heading` places a rider with the safe-spawn
//! search, seeded by `seed` (overridable with `--seed`), so one file sweeps
//! many starting layouts.
//!
//! ```toml
//! seed = 7
//! tier = "full"                    # optional: pin the bots' level of detail
//!                                  # (full | t1 | t2); default follows the rider count
//!
//! [[rider]]
//! name = "trapper"
//! bot = "trapper"                  # survivalist | trapper | adaptive |
//! difficulty = 2500                # territorialist | duelist | roamer
//! respawn_after = 240              # optional: respawn N ticks after a death
//!
//! [rider.knobs]                    # optional: override single knobs of the preset
//! reaction_ticks = 4               # (also horizon_ticks, variety, noise)
//! ```
//!
//! Matches: a `[match]` block (`shared::MatchRules`) plays rounds. The match
//! rules replace the riders' `respawn_after`; the run stops when the match
//! ends, and `until` stays the hard cap. After each round every rider is reset
//! and placed again with a new seed.
//!
//! ```toml
//! until = 9000
//!
//! [match]
//! respawn = "never"                # "never" | { after = 240 }
//! rounds = 5
//!
//! [match.end]
//! last_standing = true             # round ends with at most one rider alive
//! time_limit = 1800                # and/or after this many ticks
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use bots::config::{BotBuild, Difficulty, KnobOverrides, Personality};
use bots::schedule::Tier;
use glam::Vec2;
use serde::Deserialize;
use shared::MatchRules;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default = "default_until")]
    pub until: u32,
    #[serde(default)]
    pub arena: Option<[f32; 2]>,
    #[serde(default)]
    pub walls: Vec<[f32; 4]>,
    /// Seeds spawn placement for riders without `at`, and every respawn.
    #[serde(default)]
    pub seed: u64,
    /// Rounds, respawn policy and end condition; none keeps the free run.
    #[serde(default, rename = "match")]
    pub rules: Option<MatchRules>,
    /// Pin the bots' level-of-detail tier instead of deriving it from the
    /// number of living riders.
    #[serde(default)]
    pub tier: Option<TierPin>,
    #[serde(rename = "rider")]
    pub riders: Vec<RiderSpec>,
}

/// `Tier` in scenario spelling.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TierPin {
    Full,
    T1,
    T2,
}

impl TierPin {
    pub fn tier(self) -> Tier {
        match self {
            Self::Full => Tier::Full,
            Self::T1 => Tier::T1,
            Self::T2 => Tier::T2,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiderSpec {
    pub name: String,
    /// With `heading`, a fixed start; without, a seeded safe spawn.
    #[serde(default)]
    pub at: Option<[f32; 2]>,
    #[serde(default)]
    pub heading: Option<Heading>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_rubber")]
    pub rubber: f32,
    /// Corners of a trail the rider already has, oldest first; the last leg
    /// runs from the last corner to `at`. For puzzles: a trail to be closed in
    /// by, without scripting the ride that lays it.
    #[serde(default)]
    pub trail: Vec<[f32; 2]>,
    #[serde(default)]
    pub script: Vec<String>,
    /// Bot personality; the rider is then driven by the bot brain.
    #[serde(default)]
    pub bot: Option<String>,
    /// Authoring preset the bot's knobs start from (see `bots::config::Difficulty`).
    #[serde(default = "default_difficulty")]
    pub difficulty: u32,
    /// Per-knob overrides applied on top of the preset.
    #[serde(default)]
    pub knobs: KnobOverrides,
    /// Respawn automatically this many ticks after each death.
    #[serde(default)]
    pub respawn_after: Option<u32>,
    /// A ready-made build (tournaments); wins over `bot`/`difficulty`/`knobs`.
    #[serde(skip)]
    pub prebuilt: Option<BotBuild>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Heading {
    #[serde(rename = "+x")]
    PosX,
    #[serde(rename = "-x")]
    NegX,
    #[serde(rename = "+y")]
    PosY,
    #[serde(rename = "-y")]
    NegY,
}

impl Heading {
    pub fn vector(self) -> Vec2 {
        match self {
            Self::PosX => Vec2::X,
            Self::NegX => Vec2::NEG_X,
            Self::PosY => Vec2::Y,
            Self::NegY => Vec2::NEG_Y,
        }
    }

    /// Scenario spelling of a cardinal sim heading (`"?"` if not cardinal).
    pub fn label(direction: Vec2) -> &'static str {
        [Self::PosX, Self::NegX, Self::PosY, Self::NegY]
            .into_iter()
            .find(|heading| heading.vector() == direction)
            .map_or("?", |heading| match heading {
                Self::PosX => "+x",
                Self::NegX => "-x",
                Self::PosY => "+y",
                Self::NegY => "-y",
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Left,
    Right,
    Respawn,
}

fn default_until() -> u32 {
    600
}

fn default_speed() -> f32 {
    1.0
}

fn default_rubber() -> f32 {
    shared::BASE_RUBBER
}

fn default_difficulty() -> u32 {
    1500
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let scenario: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// Check what the TOML types cannot: names, placements, scripts, rules.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(rules) = &self.rules {
            rules.validate()?;
            if rules.end.last_standing && self.riders.len() < 2 {
                return Err("match: last_standing needs at least two riders".into());
            }
        }
        let mut names = std::collections::HashSet::new();
        for rider in &self.riders {
            if !names.insert(rider.name.as_str()) {
                return Err(format!("duplicate rider name {:?}", rider.name));
            }
            if rider.at.is_some() != rider.heading.is_some() {
                return Err(format!(
                    "{}: give both `at` and `heading`, or neither",
                    rider.name
                ));
            }
            let commands = rider.commands()?;
            if rider.bot.is_some() && commands.values().any(|a| *a != Action::Respawn) {
                return Err(format!("{}: a bot's script may only respawn", rider.name));
            }
            rider.build()?;
        }
        Ok(())
    }
}

/// A bot build in scenario vocabulary: personality name, authoring difficulty
/// and knob overrides. Suite files describe their builds the same way.
pub fn bot_build(
    name: &str,
    bot: &str,
    difficulty: u32,
    knobs: &KnobOverrides,
) -> Result<BotBuild, String> {
    let personality = match bot.to_ascii_lowercase().as_str() {
        "survivalist" => Personality::Survivalist,
        "trapper" => Personality::Trapper,
        "adaptive" => Personality::Adaptive,
        "territorialist" => Personality::Territorialist,
        "duelist" => Personality::Duelist,
        "roamer" => Personality::Roamer,
        "passive" => Personality::Passive,
        other => return Err(format!("{name}: unknown bot {other:?}")),
    };
    let mut set = Difficulty(difficulty).knobs();
    knobs.apply(&mut set);
    Ok(BotBuild::new(name, personality, set))
}

impl RiderSpec {
    /// The bot build for a `bot` rider, `None` for a scripted one.
    pub fn build(&self) -> Result<Option<BotBuild>, String> {
        if let Some(build) = &self.prebuilt {
            return Ok(Some(build.clone()));
        }
        let Some(bot) = &self.bot else {
            return Ok(None);
        };
        bot_build(&self.name, bot, self.difficulty, &self.knobs).map(Some)
    }

    /// Parse `"<tick> <action>"` entries into one action per tick. The sim
    /// applies at most one turn per tick, so two entries on a tick is an error.
    pub fn commands(&self) -> Result<BTreeMap<u32, Action>, String> {
        let mut commands = BTreeMap::new();
        for entry in &self.script {
            let mut parts = entry.split_whitespace();
            let (Some(tick), Some(action), None) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(format!(
                    "{}: expected \"<tick> <action>\", got {entry:?}",
                    self.name
                ));
            };
            let tick: u32 = tick
                .parse()
                .map_err(|_| format!("{}: bad tick in {entry:?}", self.name))?;
            let action = match action.to_ascii_lowercase().as_str() {
                "l" | "left" => Action::Left,
                "r" | "right" => Action::Right,
                "respawn" => Action::Respawn,
                other => return Err(format!("{}: unknown action {other:?}", self.name)),
            };
            if commands.insert(tick, action).is_some() {
                return Err(format!("{}: two actions on tick {tick}", self.name));
            }
        }
        Ok(commands)
    }
}
