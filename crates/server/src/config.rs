//! Game settings of a server, from a TOML file: the room, the match and round
//! rules, and the bots. `SERVER_CONFIG=path` loads a file; without it the
//! server runs the official config embedded at build time
//! (`config/official.toml` in this crate). Deployment settings (ports,
//! advertised host, manager, secrets) stay in env vars.
//!
//! ```toml
//! [server]
//! name = "Tron Zero"
//! max_players = 10            # humans; bots do not count
//!
//! [match]
//! rounds = 5
//! intermission_secs = 3       # before round 1 and between rounds
//! end_screen_secs = 8         # after the last round, showing the scores
//!
//! [round]
//! secs = 90                   # 0: no time limit
//! respawn = "next_round"      # or "anytime"
//! last_standing = true        # needs respawn = "next_round"
//! end_without_humans = true   # optional, true by default
//!
//! [bots]
//! roster = "builtin"          # or a roster file, relative to this file
//! ```
//!
//! Every key is required unless marked optional. An invalid file stops the
//! server.

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
    #[serde(rename = "match")]
    pub game: Match,
    pub round: Round,
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

/// A match: its rounds and the breaks around them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    pub rounds: u32,
    pub intermission_secs: u32,
    pub end_screen_secs: u32,
}

/// One round: its length, respawns and end conditions.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Round {
    pub secs: u32,
    pub respawn: RespawnRule,
    pub last_standing: bool,
    #[serde(default = "yes")]
    pub end_without_humans: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RespawnRule {
    /// A dead rider comes back during the round.
    Anytime,
    /// A dead rider waits for the next round.
    NextRound,
}

fn yes() -> bool {
    true
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

    /// The match rules. A match always needs a human: bots alone never play.
    /// Rules that cannot run are an error.
    pub fn rules(&self) -> Result<MatchRules, String> {
        let respawn = match self.round.respawn {
            RespawnRule::Anytime => Respawn::After(crate::bot::RESPAWN_DELAY_TICKS),
            RespawnRule::NextRound => Respawn::Never,
        };
        let rules = MatchRules {
            respawn,
            end: EndRules {
                time_limit: (self.round.secs > 0).then(|| secs_to_ticks(self.round.secs)),
                last_standing: self.round.last_standing,
                no_human_left: self.round.end_without_humans,
            },
            rounds: self.game.rounds,
            intermission: secs_to_ticks(self.game.intermission_secs),
            final_intermission: secs_to_ticks(self.game.end_screen_secs),
            needs_human: true,
        };
        rules.validate()?;
        if rules.intermission == 0 || rules.final_intermission == 0 {
            return Err("intermission_secs and end_screen_secs must be at least 1".into());
        }
        Ok(rules)
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
    fn every_shipped_config_and_its_roster_load() {
        for text in [
            include_str!("../config/official.toml"),
            include_str!("../config/local.toml"),
            include_str!("../config/dev-cloud.toml"),
        ] {
            let config = ServerConfig::parse(text, Path::new("")).unwrap();
            config.rules().unwrap();
            assert!(!config.roster().unwrap().is_empty());
        }
    }

    #[test]
    fn rules_come_from_the_file_and_bad_ones_are_refused() {
        let file = |round: &str| {
            let text = format!(
                "[server]\nname = \"t\"\nmax_players = 4\n\
                 [match]\nrounds = 7\nintermission_secs = 3\nend_screen_secs = 8\n\
                 [round]\n{round}\n[bots]\nroster = \"builtin\"\n"
            );
            ServerConfig::parse(&text, Path::new(""))
        };
        let rounds = "secs = 0\nrespawn = \"next_round\"\nlast_standing = true";
        let rules = file(rounds).unwrap().rules().unwrap();
        assert_eq!((rules.rounds, rules.end.time_limit), (7, None));
        assert!(rules.end.no_human_left && rules.needs_human);
        let off = file(&format!("{rounds}\nend_without_humans = false")).unwrap();
        assert!(!off.rules().unwrap().end.no_human_left);
        // Last standing with respawns could never end a round.
        let bad = rounds.replace("next_round", "anytime");
        assert!(file(&bad).unwrap().rules().is_err());
        assert!(file("secs = 90").is_err());
        assert!(file(&rounds.replace("next_round", "sometimes")).is_err());
    }
}
