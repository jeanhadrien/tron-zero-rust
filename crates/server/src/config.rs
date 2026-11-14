//! Game settings of a server, from a TOML file: the room, the game mode and the
//! bots. `SERVER_CONFIG=path` loads a file; without it the server runs the
//! official config embedded at build time (`config/official.toml` in this
//! crate). Deployment settings (ports, advertised host, manager, secrets)
//! stay in env vars.
//!
//! ```toml
//! [server]
//! name = "Tron Zero"
//! max_players = 10            # humans; bots do not count
//!
//! [game]
//! mode = "rounds"             # or "ffa" (no other key then)
//! rounds = 5
//! round_secs = 90             # 0: no time limit
//! intermission_secs = 3
//! final_intermission_secs = 8
//!
//! [bots]
//! roster = "builtin"          # or a roster file, relative to this file
//! ```
//!
//! Every key is required. An invalid file stops the server.

use std::path::{Path, PathBuf};

use bots::config::BotBuild;
use serde::Deserialize;
use shared::match_rules::secs_to_ticks;
use shared::{EndRules, MatchRules, Respawn};

/// Env var naming the config file.
pub const CONFIG_ENV: &str = "SERVER_CONFIG";
/// The `roster` value that names the roster embedded in the bots crate.
const BUILTIN: &str = "builtin";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub server: Room,
    pub game: Game,
    pub bots: Bots,
    /// Directory relative paths resolve against: the file's.
    #[serde(skip)]
    dir: PathBuf,
}

/// The room as listed in the lobby.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    pub name: String,
    pub max_players: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum Game {
    /// Free-for-all: free spawns, manual respawns, no end.
    Ffa,
    /// Last rider standing per round (`docs/specs/rounds-mode.md`).
    Rounds {
        rounds: u32,
        round_secs: u32,
        intermission_secs: u32,
        final_intermission_secs: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bots {
    pub roster: String,
}

impl ServerConfig {
    /// The file named by `SERVER_CONFIG`, else the official one.
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(CONFIG_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            Some(path) => Self::load(Path::new(path.trim())),
            None => Self::official(),
        }
    }

    /// The official config, embedded at build time.
    pub fn official() -> Result<Self, String> {
        Self::parse(include_str!("../config/official.toml"), Path::new(""))
            .map_err(|e| format!("official config: {e}"))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new(""));
        Self::parse(&text, dir).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str, dir: &Path) -> Result<Self, String> {
        let mut config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        config.dir = dir.to_path_buf();
        Ok(config)
    }

    /// The match rules of rounds mode, `None` for the free-for-all. Rules that
    /// cannot run are an error.
    pub fn rules(&self) -> Result<Option<MatchRules>, String> {
        let Game::Rounds {
            rounds,
            round_secs,
            intermission_secs,
            final_intermission_secs,
        } = self.game
        else {
            return Ok(None);
        };
        let rules = MatchRules {
            respawn: Respawn::Never,
            end: EndRules {
                time_limit: (round_secs > 0).then(|| secs_to_ticks(round_secs)),
                last_standing: true,
            },
            rounds,
            intermission: secs_to_ticks(intermission_secs),
            final_intermission: secs_to_ticks(final_intermission_secs),
        };
        rules.validate()?;
        if rules.intermission == 0 || rules.final_intermission == 0 {
            return Err("intermissions must last at least 1 second".into());
        }
        Ok(Some(rules))
    }

    /// The builds to seat, one bot each.
    pub fn roster(&self) -> Result<Vec<BotBuild>, String> {
        match self.bots.roster.as_str() {
            BUILTIN => bots::roster::builtin(),
            path => bots::roster::load(&self.dir.join(path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_official_config_and_its_roster_load() {
        let config = ServerConfig::official().unwrap();
        config.rules().unwrap();
        assert!(!config.roster().unwrap().is_empty());
    }

    #[test]
    fn rounds_rules_come_from_the_file_and_bad_ones_are_refused() {
        let file = |game: &str| {
            let text = format!(
                "[server]\nname = \"t\"\nmax_players = 4\n[game]\n{game}\n[bots]\nroster = \"builtin\"\n"
            );
            ServerConfig::parse(&text, Path::new(""))
        };
        assert_eq!(file("mode = \"ffa\"").unwrap().rules(), Ok(None));
        let rounds = "mode = \"rounds\"\nrounds = 7\nround_secs = 0\nintermission_secs = 3\nfinal_intermission_secs = 8";
        let rules = file(rounds).unwrap().rules().unwrap().unwrap();
        assert_eq!(rules.rounds, 7);
        assert_eq!(rules.end.time_limit, None);
        assert!(file("mode = \"tdm\"").is_err());
        assert!(file("mode = \"rounds\"\nrounds = 5").is_err());
        let zero = rounds.replace("rounds = 7", "rounds = 0");
        assert!(file(&zero).unwrap().rules().is_err());
    }
}
