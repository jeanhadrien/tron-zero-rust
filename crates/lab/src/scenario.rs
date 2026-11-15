//! Scenario files: a scene (`scene::Scene`: walls, riders, scripts, bots) plus
//! what only the lab needs to run it. The sim is deterministic, so exploring a
//! situation means editing the file and re-running from tick 0.
//!
//! ```toml
//! until = 600                      # ticks to simulate (120 per second)
//! arena = [2400, 2400]             # optional, defaults to the game arena
//! walls = [[-200, 100, 200, 100]]  # scene: optional extra segments x1 y1 x2 y2
//!
//! [[rider]]                        # scene riders, see the `scene` crate
//! name = "a"
//! at = [-1000, 0]
//! heading = "+x"
//! script = ["40 left", "100 right", "300 respawn"]
//! respawn_after = 240              # lab: optional, respawn N ticks after a death
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

use std::path::Path;

use bots::schedule::Tier;
use scene::Scene;
use serde::Deserialize;
use shared::MatchRules;
use toml::{Table, Value};

pub use scene::{Action, Heading};

#[derive(Debug)]
pub struct Scenario {
    pub until: u32,
    pub arena: Option<[f32; 2]>,
    /// Seeds spawn placement for riders without `at`, and every respawn.
    pub seed: u64,
    /// Rounds, respawn policy and end condition; none keeps the free run.
    pub rules: Option<MatchRules>,
    /// Pin the bots' level-of-detail tier instead of deriving it from the
    /// number of living riders.
    pub tier: Option<TierPin>,
    pub scene: Scene,
    /// Per scene rider: respawn automatically this many ticks after each death.
    pub respawn_after: Vec<Option<u32>>,
}

/// The keys of a scenario file that are not scene keys.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabKeys {
    #[serde(default = "default_until")]
    until: u32,
    #[serde(default)]
    arena: Option<[f32; 2]>,
    #[serde(default)]
    seed: u64,
    #[serde(default, rename = "match")]
    rules: Option<MatchRules>,
    #[serde(default)]
    tier: Option<TierPin>,
}

const LAB_KEYS: [&str; 5] = ["until", "arena", "seed", "match", "tier"];
const LAB_RIDER_KEY: &str = "respawn_after";

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

fn default_until() -> u32 {
    600
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Split the file into lab keys and a scene, each checked for unknown
    /// keys, then check what the TOML types cannot.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut table: Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut lab = Table::new();
        for key in LAB_KEYS {
            if let Some(value) = table.remove(key) {
                lab.insert(key.into(), value);
            }
        }
        let mut respawn_after = Vec::new();
        if let Some(Value::Array(riders)) = table.get_mut("rider") {
            for rider in riders {
                let value = match rider {
                    Value::Table(rider) => rider.remove(LAB_RIDER_KEY),
                    _ => None,
                };
                let after = value
                    .map(|v| v.try_into::<u32>())
                    .transpose()
                    .map_err(|e| format!("{LAB_RIDER_KEY}: {e}"))?;
                respawn_after.push(after);
            }
        }
        let keys: LabKeys = Value::Table(lab)
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        let scenario = Self {
            until: keys.until,
            arena: keys.arena,
            seed: keys.seed,
            rules: keys.rules,
            tier: keys.tier,
            scene: Scene::from_table(table, scene::Time::Ticks)?,
            respawn_after,
        };
        scenario.validate()?;
        Ok(scenario)
    }

    fn validate(&self) -> Result<(), String> {
        if let Some(rules) = &self.rules {
            rules.validate()?;
            if rules.end.last_standing && self.scene.riders.len() < 2 {
                return Err("match: last_standing needs at least two riders".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_and_scene_keys_split_and_typos_still_fail() {
        let text = "until = 10\nseed = 3\n[[rider]]\nname = \"a\"\nrespawn_after = 5\n[[rider]]\nname = \"b\"";
        let scenario = Scenario::parse(text).unwrap();
        assert_eq!((scenario.until, scenario.seed), (10, 3));
        assert_eq!(scenario.respawn_after, [Some(5), None]);
        assert_eq!(scenario.scene.riders.len(), 2);
        assert!(Scenario::parse("unitl = 10\n[[rider]]\nname = \"a\"").is_err());
        assert!(Scenario::parse("[[rider]]\nname = \"a\"\nrespawn_afer = 5").is_err());
    }
}
