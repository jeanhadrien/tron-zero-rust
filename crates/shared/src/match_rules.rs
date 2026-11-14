//! Match rules: how a game of several rounds is played and when it ends.
//!
//! Plain data and pure decisions, outside the simulation: nothing here is read
//! by `simulate_players`, replicated or part of `PROTOCOL_ID`. [`MatchRules`]
//! is the data, [`MatchFlow`] the state machine that plays a match by it
//! (`docs/specs/rounds-mode.md`). Later modes (first to K kills, teams) are new
//! fields of `EndRules` and new `RoundEnd` variants, not a new type.

use bevy_ecs::entity::Entity;
use serde::{Deserialize, Serialize};

use crate::MatchPhase;
use crate::constants::TICK_HZ;

/// Riders a match needs to run: with fewer, nobody is playing anybody.
const MIN_RIDERS: usize = 2;

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
    /// Ticks between two rounds; 0 chains them with no break.
    #[serde(default = "default_intermission")]
    pub intermission: u32,
    /// Ticks between the last round and the next match, showing the standings.
    #[serde(default = "default_final_intermission")]
    pub final_intermission: u32,
}

/// `secs` seconds in ticks.
pub fn secs_to_ticks(secs: u32) -> u32 {
    (f64::from(secs) * TICK_HZ) as u32
}

fn default_intermission() -> u32 {
    secs_to_ticks(3)
}

fn default_final_intermission() -> u32 {
    secs_to_ticks(8)
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
            intermission: default_intermission(),
            final_intermission: default_final_intermission(),
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

/// What [`MatchFlow`] decided for one tick; the caller applies it to its world.
#[derive(Debug, PartialEq, Eq)]
pub enum MatchStep {
    Idle,
    /// A second rider is here: stop everybody, count down to round 1.
    Starting,
    /// Place every rider and start a round; `new_match` resets the scores.
    Begin {
        new_match: bool,
    },
    /// The round is over, for `reason`: `winner` is the last rider alive, if
    /// there is one.
    RoundOver {
        reason: RoundEnd,
        winner: Option<Entity>,
    },
}

/// The state machine of a match: phases, round counter and clock, decided by
/// [`MatchRules`]. Pure and counted in fixed ticks; the caller applies each
/// [`MatchStep`] to its riders.
#[derive(Debug)]
pub struct MatchFlow {
    rules: MatchRules,
    phase: MatchPhase,
    /// Current round from 1; 0 while `Free`.
    round: u32,
    /// Ticks spent in this phase.
    ticks: u32,
}

impl MatchFlow {
    /// Idle until two riders are here, then a countdown and round 1.
    pub fn new(rules: MatchRules) -> Self {
        Self {
            rules,
            phase: MatchPhase::Free,
            round: 0,
            ticks: 0,
        }
    }

    /// Already playing round 1, its riders placed by the caller.
    pub fn in_round(rules: MatchRules) -> Self {
        Self {
            phase: MatchPhase::Playing,
            round: 1,
            ..Self::new(rules)
        }
    }

    pub fn phase(&self) -> MatchPhase {
        self.phase
    }

    /// Current round, from 1; 0 while `Free`.
    pub fn round(&self) -> u32 {
        self.round
    }

    pub fn rules(&self) -> &MatchRules {
        &self.rules
    }

    /// Whether a match is on. When false the riders play free: normal respawns.
    pub fn is_running(&self) -> bool {
        self.phase != MatchPhase::Free
    }

    /// Advance one fixed tick, after the tick's deaths. `riders` counts
    /// everybody in the arena roster, `alive` those still riding.
    pub fn advance(&mut self, riders: usize, alive: &[Entity]) -> MatchStep {
        match self.phase {
            MatchPhase::Free => {
                if riders < MIN_RIDERS {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Starting, 1);
                MatchStep::Starting
            }
            MatchPhase::Starting => {
                if !self.wait(riders, self.rules.intermission) {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Playing, 1);
                MatchStep::Begin { new_match: true }
            }
            MatchPhase::Playing => {
                self.ticks += 1;
                let Some(end) = self.rules.round_end(self.ticks, alive.len()) else {
                    return MatchStep::Idle;
                };
                let winner = match (end, alive) {
                    (RoundEnd::LastStanding, &[only]) => Some(only),
                    _ => None,
                };
                let phase = if self.round >= self.rules.rounds {
                    MatchPhase::MatchOver
                } else {
                    MatchPhase::Intermission
                };
                self.enter(phase, self.round);
                MatchStep::RoundOver {
                    reason: end,
                    winner,
                }
            }
            MatchPhase::Intermission => {
                if !self.wait(riders, self.rules.intermission) {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Playing, self.round + 1);
                MatchStep::Begin { new_match: false }
            }
            MatchPhase::MatchOver => {
                if !self.wait(riders, self.rules.final_intermission) {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Playing, 1);
                MatchStep::Begin { new_match: true }
            }
        }
    }

    fn enter(&mut self, phase: MatchPhase, round: u32) {
        self.phase = phase;
        self.round = round;
        self.ticks = 0;
    }

    /// Count a break tick; true once it has lasted `len` ticks. A break of 0
    /// ticks is over at once, whoever is left. With too few riders left during
    /// a real break the match is dropped and the riders play free.
    fn wait(&mut self, riders: usize, len: u32) -> bool {
        if len == 0 {
            return true;
        }
        if riders < MIN_RIDERS {
            self.enter(MatchPhase::Free, 0);
            return false;
        }
        self.ticks += 1;
        self.ticks >= len
    }

    /// Ticks the current phase's clock lasts, `None` without a limit.
    fn clock_len(&self) -> Option<u32> {
        match self.phase {
            MatchPhase::Free => None,
            MatchPhase::Playing => self.rules.end.time_limit,
            MatchPhase::Starting | MatchPhase::Intermission => Some(self.rules.intermission),
            MatchPhase::MatchOver => Some(self.rules.final_intermission),
        }
    }

    /// Whole seconds left on this phase's clock, `None` without a limit.
    pub fn secs_left(&self) -> Option<u16> {
        self.clock_len().map(|len| {
            let left = len.saturating_sub(self.ticks).div_ceil(TICK_HZ as u32);
            left.min(u32::from(u16::MAX)) as u16
        })
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
            ..MatchRules::default()
        };
        assert_eq!(rules.round_end(99, 0), None);
        assert_eq!(rules.round_end(100, 4), Some(RoundEnd::TimeLimit));
    }

    fn rounds_of(rounds: u32, limit: Option<u32>) -> MatchRules {
        MatchRules {
            respawn: Respawn::Never,
            end: EndRules {
                time_limit: limit,
                last_standing: true,
            },
            rounds,
            intermission: 3,
            final_intermission: 5,
        }
    }

    fn entities(n: u32) -> Vec<Entity> {
        (1..=n)
            .map(Entity::from_raw_u32)
            .map(Option::unwrap)
            .collect()
    }

    /// Run `ticks` idle ticks with everyone alive.
    fn idle(flow: &mut MatchFlow, riders: &[Entity], ticks: u32) {
        for _ in 0..ticks {
            assert_eq!(flow.advance(riders.len(), riders), MatchStep::Idle);
        }
    }

    /// Bring a fresh match to the first tick of round 1.
    fn playing(flow: &mut MatchFlow, riders: &[Entity]) {
        assert_eq!(flow.advance(riders.len(), riders), MatchStep::Starting);
        idle(flow, riders, 2);
        assert_eq!(
            flow.advance(riders.len(), riders),
            MatchStep::Begin { new_match: true }
        );
        assert_eq!(flow.phase(), MatchPhase::Playing);
    }

    #[test]
    fn fewer_than_two_riders_keep_the_match_free() {
        let mut flow = MatchFlow::new(rounds_of(2, None));
        let riders = entities(1);
        idle(&mut flow, &riders, 10);
        assert!(!flow.is_running());
        assert_eq!(flow.advance(0, &[]), MatchStep::Idle);
    }

    #[test]
    fn a_second_rider_starts_a_countdown_then_round_one() {
        let mut flow = MatchFlow::new(rounds_of(2, None));
        playing(&mut flow, &entities(2));
        assert!(flow.is_running());
        assert_eq!(flow.round(), 1);
    }

    #[test]
    fn the_last_rider_alive_wins_and_the_break_precedes_the_next_round() {
        let mut flow = MatchFlow::new(rounds_of(3, None));
        let riders = entities(3);
        playing(&mut flow, &riders);
        idle(&mut flow, &riders, 5);
        // Two of three die.
        assert_eq!(
            flow.advance(3, &riders[..1]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: Some(riders[0])
            }
        );
        assert_eq!((flow.phase(), flow.round()), (MatchPhase::Intermission, 1));
        // The break lasts `intermission` ticks, then round 2 begins.
        idle(&mut flow, &riders, 2);
        assert_eq!(
            flow.advance(3, &riders),
            MatchStep::Begin { new_match: false }
        );
        assert_eq!((flow.phase(), flow.round()), (MatchPhase::Playing, 2));
    }

    #[test]
    fn nobody_wins_when_the_last_riders_die_together_or_time_runs_out() {
        let mut flow = MatchFlow::new(rounds_of(3, Some(10)));
        let riders = entities(3);
        playing(&mut flow, &riders);
        assert_eq!(
            flow.advance(3, &[]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: None
            }
        );
        idle(&mut flow, &riders, 2);
        assert_eq!(
            flow.advance(3, &riders),
            MatchStep::Begin { new_match: false }
        );
        // Time limit with several alive: round 2 lasts exactly 10 ticks.
        idle(&mut flow, &riders, 9);
        assert_eq!(
            flow.advance(3, &riders),
            MatchStep::RoundOver {
                reason: RoundEnd::TimeLimit,
                winner: None
            }
        );
    }

    #[test]
    fn the_last_round_leads_to_the_standings_then_a_new_match() {
        let mut flow = MatchFlow::new(rounds_of(1, None));
        let riders = entities(2);
        playing(&mut flow, &riders);
        assert_eq!(
            flow.advance(2, &riders[1..]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: Some(riders[1])
            }
        );
        assert_eq!(flow.phase(), MatchPhase::MatchOver);
        idle(&mut flow, &riders, 4);
        assert_eq!(
            flow.advance(2, &riders),
            MatchStep::Begin { new_match: true }
        );
        assert_eq!((flow.phase(), flow.round()), (MatchPhase::Playing, 1));
    }

    #[test]
    fn a_leaver_counts_as_eliminated_and_too_few_riders_end_the_match() {
        let mut flow = MatchFlow::new(rounds_of(3, None));
        let riders = entities(2);
        playing(&mut flow, &riders);
        // One rider left the arena: one alive, the survivor wins.
        assert_eq!(
            flow.advance(1, &riders[..1]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: Some(riders[0])
            }
        );
        // Still alone after the break started: back to free riding.
        assert_eq!(flow.advance(1, &riders[..1]), MatchStep::Idle);
        assert!(!flow.is_running());
        assert_eq!(flow.round(), 0);
    }

    #[test]
    fn a_match_with_no_break_chains_its_rounds_from_round_one() {
        let rules = MatchRules {
            intermission: 0,
            end: EndRules {
                time_limit: Some(4),
                last_standing: false,
            },
            ..rounds_of(2, None)
        };
        let mut flow = MatchFlow::in_round(rules);
        // One rider is enough: a zero break never drops the match.
        let riders = entities(1);
        idle(&mut flow, &riders, 3);
        assert_eq!(
            flow.advance(1, &riders),
            MatchStep::RoundOver {
                reason: RoundEnd::TimeLimit,
                winner: None
            }
        );
        assert_eq!(
            flow.advance(1, &riders),
            MatchStep::Begin { new_match: false }
        );
        assert_eq!((flow.phase(), flow.round()), (MatchPhase::Playing, 2));
        idle(&mut flow, &riders, 3);
        assert_eq!(
            flow.advance(1, &riders),
            MatchStep::RoundOver {
                reason: RoundEnd::TimeLimit,
                winner: None
            }
        );
        assert_eq!(flow.phase(), MatchPhase::MatchOver);
    }

    #[test]
    fn the_clock_counts_down_in_whole_seconds() {
        let mut flow = MatchFlow::new(rounds_of(2, Some(secs_to_ticks(90))));
        assert_eq!(flow.secs_left(), None);
        let riders = entities(2);
        flow.advance(2, &riders);
        assert_eq!(flow.secs_left(), Some(1));
        flow.enter(MatchPhase::Free, 0);
        playing(&mut flow, &riders);
        assert_eq!(flow.secs_left(), Some(90));
        idle(&mut flow, &riders, secs_to_ticks(1));
        assert_eq!(flow.secs_left(), Some(89));
    }
}
