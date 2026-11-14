//! Match rules: how a game of several rounds is played and when it ends.
//!
//! Plain data and pure decisions, outside the simulation: nothing here is read
//! by `simulate_players`, replicated or part of `PROTOCOL_ID`. The lab plays
//! matches with it today; the game server is meant to adopt the same type for
//! an in-game rounds mode. Later modes (first to K kills, teams) are new
//! fields of `EndRules` and new `RoundEnd` variants, not a new type.

use serde::{Deserialize, Serialize};

/// What happens to a rider after its death.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Respawn {
    /// Dead until the round ends (last rider standing).
    #[default]
    Never,
    /// Back in the arena this many ticks after the death (free-for-all).
    After(u32),
}

impl Respawn {
    /// Ticks between a death and its respawn, `None` when riders stay dead.
    pub fn delay(self) -> Option<u32> {
        match self {
            Self::Never => None,
            Self::After(ticks) => Some(ticks),
        }
    }
}

/// When a round stops. Conditions combine: the first one met ends the round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndRules {
    /// Round length in ticks.
    #[serde(default)]
    pub time_limit: Option<u32>,
    /// End when at most one rider is alive; the survivor wins the round.
    #[serde(default)]
    pub last_standing: bool,
}

/// Why a round ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundEnd {
    /// At most one rider alive; with exactly one alive it is the winner.
    LastStanding,
    TimeLimit,
}

impl RoundEnd {
    /// Spelling used in traces and reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::LastStanding => "last_standing",
            Self::TimeLimit => "time_limit",
        }
    }
}

/// One match: a respawn policy, an end condition and a number of rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRules {
    #[serde(default)]
    pub respawn: Respawn,
    #[serde(default)]
    pub end: EndRules,
    #[serde(default = "one_round")]
    pub rounds: u32,
}

fn one_round() -> u32 {
    1
}

impl Default for MatchRules {
    fn default() -> Self {
        Self {
            respawn: Respawn::default(),
            end: EndRules::default(),
            rounds: one_round(),
        }
    }
}

impl MatchRules {
    /// Whether a round that began `elapsed` ticks ago, with `alive` riders
    /// still standing, is over, and why.
    pub fn round_end(&self, elapsed: u32, alive: usize) -> Option<RoundEnd> {
        if self.end.last_standing && alive <= 1 {
            return Some(RoundEnd::LastStanding);
        }
        match self.end.time_limit {
            Some(limit) if elapsed >= limit => Some(RoundEnd::TimeLimit),
            _ => None,
        }
    }

    /// Reject rules no round could ever end under.
    pub fn validate(&self) -> Result<(), String> {
        if self.rounds == 0 {
            return Err("match: rounds must be at least 1".into());
        }
        if self.end.time_limit == Some(0) {
            return Err("match: time_limit must be at least 1 tick".into());
        }
        if self.end.time_limit.is_none() && !self.end.last_standing {
            return Err("match: give a time_limit and/or last_standing".into());
        }
        if self.end.last_standing && self.respawn != Respawn::Never {
            return Err("match: last_standing needs respawn = \"never\"".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_standing_ends_on_one_or_none_alive() {
        let rules = MatchRules {
            end: EndRules {
                time_limit: Some(100),
                last_standing: true,
            },
            ..MatchRules::default()
        };
        assert_eq!(rules.round_end(10, 3), None);
        assert_eq!(rules.round_end(10, 1), Some(RoundEnd::LastStanding));
        assert_eq!(rules.round_end(10, 0), Some(RoundEnd::LastStanding));
        assert_eq!(rules.round_end(100, 3), Some(RoundEnd::TimeLimit));
    }

    #[test]
    fn time_limit_alone_ignores_deaths() {
        let rules = MatchRules {
            respawn: Respawn::After(240),
            end: EndRules {
                time_limit: Some(100),
                last_standing: false,
            },
            rounds: 1,
        };
        assert_eq!(rules.round_end(99, 0), None);
        assert_eq!(rules.round_end(100, 4), Some(RoundEnd::TimeLimit));
    }
}
