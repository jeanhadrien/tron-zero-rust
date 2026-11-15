//! A bot build in file vocabulary: a personality, a difficulty preset and
//! knob or weight overrides by name. Suites, tune, ladder and scene files all
//! write builds this way.
//!
//! ```toml
//! name = "Tuned"
//! bot = "trapper"            # personality, lowercase; `personality` also works
//! difficulty = 2500          # optional: the authoring preset the knobs start from
//! [knobs]                    # optional: any knob of `config::BotKnobs`
//! noise = 0.0
//! [weights]                  # optional: any weight of `objective::Weights`
//! pressure = 3.0
//! ```

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use toml::{Table, Value};

use crate::config::{BotBuild, BotKnobs, Difficulty, Personality};

/// The preset a build without `difficulty` starts from.
pub const DEFAULT_DIFFICULTY: u32 = 1500;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    pub name: String,
    #[serde(alias = "personality")]
    pub bot: Personality,
    #[serde(default = "default_difficulty")]
    pub difficulty: u32,
    #[serde(default)]
    pub knobs: Table,
    #[serde(default)]
    pub weights: Table,
}

fn default_difficulty() -> u32 {
    DEFAULT_DIFFICULTY
}

/// `base` with the entries of `over` replacing same-named fields. Unknown
/// names and wrongly typed values are errors.
pub fn merged<T: Serialize + DeserializeOwned>(
    base: &T,
    over: &Table,
    what: &str,
) -> Result<T, String> {
    let mut table = Table::try_from(base).map_err(|e| e.to_string())?;
    for (key, value) in over {
        if !table.contains_key(key) {
            let known: Vec<&String> = table.keys().collect();
            return Err(format!("unknown {what} {key:?}, known: {known:?}"));
        }
        table.insert(key.clone(), value.clone());
    }
    Value::Table(table)
        .try_into()
        .map_err(|e| format!("{what}: {e}"))
}

impl BuildSpec {
    /// The build, with the planner tuning of the built-in roster.
    pub fn build(&self) -> Result<BotBuild, String> {
        let mut knobs: BotKnobs = merged(&Difficulty(self.difficulty).knobs(), &self.knobs, "knob")
            .map_err(|e| format!("{}: {e}", self.name))?;
        knobs.clamp();
        let tuning = crate::roster::builtin_tuning()?;
        let mut build = BotBuild::new(&self.name, self.bot, knobs, tuning);
        build.weights = merged(&build.weights, &self.weights, "weight")
            .map_err(|e| format!("{}: {e}", self.name))?;
        Ok(build)
    }
}
