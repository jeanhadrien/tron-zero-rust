//! Replicated state of the match (server-authoritative display data).
//!
//! Never read by `simulate_players`: the match is decided by `MatchFlow` and
//! only shown by clients.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::{DeathCause, DeathRecord};

/// Where the match is, as clients see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum MatchPhase {
    /// No match runs: waiting for players. Humans ride free with manual
    /// respawns; nobody else rides.
    #[default]
    Free,
    /// Countdown before round 1 of a match; nobody is riding.
    Starting,
    /// A round is being played.
    Playing,
    /// A round just ended; `winner` of [`MatchStatus`] says who took it.
    Intermission,
    /// The last round ended; the scores are final.
    MatchOver,
}

/// The single match entity of a server.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
pub struct MatchStatus {
    /// Rounds per match.
    pub rounds: u32,
    /// Current (or, during `Starting`, upcoming) round, from 1; 0 when `Free`.
    pub round: u32,
    pub phase: MatchPhase,
    /// Whole seconds left on the clock of this phase (the round time limit
    /// while `Playing`, the break otherwise); `None` without a limit.
    pub secs_left: Option<u16>,
    /// Winner of the round that just ended; `None` for a draw. Only meaningful
    /// in `Intermission` and `MatchOver`.
    pub winner: Option<String>,
    /// Whether a dead rider may respawn during a round of this match.
    pub respawns: bool,
}

/// A rider's record this match, reset when a new match starts. Replicated on
/// the rider for the scoreboard.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect,
)]
pub struct Score {
    pub kills: u32,
    pub deaths: u32,
    /// Rounds won.
    pub wins: u32,
}

/// One line of a rider's [`Score`] earned on a death.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credit {
    Kill,
    Death,
}

impl Score {
    pub fn add(&mut self, credit: Credit) {
        match credit {
            Credit::Kill => self.kills += 1,
            Credit::Death => self.deaths += 1,
        }
    }

    /// What one tick's deaths earn: each victim a death, the owner of the
    /// trail it hit a kill (both riders of a mutual crash get one of each).
    /// A wall or its own trail is a death only.
    pub fn credits(records: &[DeathRecord]) -> impl Iterator<Item = (Entity, Credit)> + '_ {
        records.iter().flat_map(|record| {
            let kill = match record.cause {
                DeathCause::Trail(killer) if killer != record.victim => {
                    Some((killer, Credit::Kill))
                }
                _ => None,
            };
            std::iter::once((record.victim, Credit::Death)).chain(kill)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mutual_crash_is_a_kill_and_a_death_for_both_and_a_wall_only_a_death() {
        let [a, b, c] = [1, 2, 3].map(|i| Entity::from_raw_u32(i).unwrap());
        let records = [
            DeathRecord {
                tick: None,
                victim: a,
                cause: DeathCause::Trail(b),
            },
            DeathRecord {
                tick: None,
                victim: b,
                cause: DeathCause::Trail(a),
            },
            DeathRecord {
                tick: None,
                victim: c,
                cause: DeathCause::Wall,
            },
        ];
        let mut scores = [Score::default(); 3];
        for (entity, credit) in Score::credits(&records) {
            scores[entity.index_u32() as usize - 1].add(credit);
        }
        let line = |kills, deaths| Score {
            kills,
            deaths,
            wins: 0,
        };
        assert_eq!(scores, [line(1, 1), line(1, 1), line(0, 1)]);
    }
}
