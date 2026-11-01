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
//!
//! [[rider]]
//! name = "trapper"
//! bot = "trapper"                  # survivalist | trapper | adaptive |
//! elo = 2500                       # territorialist | duelist | roamer
//! respawn_after = 240              # optional: respawn N ticks after a death
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use bots::config::{BotBuild, Elo, Personality};
use glam::Vec2;
use serde::Deserialize;

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
    #[serde(rename = "rider")]
    pub riders: Vec<RiderSpec>,
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
    #[serde(default)]
    pub script: Vec<String>,
    /// Bot personality; the rider is then driven by the bot brain.
    #[serde(default)]
    pub bot: Option<String>,
    #[serde(default = "default_elo")]
    pub elo: u32,
    /// Respawn automatically this many ticks after each death.
    #[serde(default)]
    pub respawn_after: Option<u32>,
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

fn default_elo() -> u32 {
    1500
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let scenario: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut names = std::collections::HashSet::new();
        for rider in &scenario.riders {
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
        Ok(scenario)
    }
}

impl RiderSpec {
    /// The bot build for a `bot` rider, `None` for a scripted one.
    pub fn build(&self) -> Result<Option<BotBuild>, String> {
        let Some(name) = &self.bot else {
            return Ok(None);
        };
        let personality = match name.to_ascii_lowercase().as_str() {
            "survivalist" => Personality::Survivalist,
            "trapper" => Personality::Trapper,
            "adaptive" => Personality::Adaptive,
            "territorialist" => Personality::Territorialist,
            "duelist" => Personality::Duelist,
            "roamer" => Personality::Roamer,
            other => return Err(format!("{}: unknown bot {other:?}", self.name)),
        };
        Ok(Some(BotBuild::new(Elo(self.elo), personality)))
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
