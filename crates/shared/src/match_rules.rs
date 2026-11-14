//! Match rules: how a match of one or more rounds is played and when it ends.
//!
//! Plain data and pure decisions, outside the simulation: nothing here is read
//! by `simulate_players`, replicated or part of `PROTOCOL_ID`. [`MatchRules`]
//! is the data, [`MatchFlow`] the state machine that plays a match by it
//! (`docs/specs/match-rules.md`). Every way to play is a set of rules: one long
//! round with respawns, or several last-standing rounds. Later rules (first to
//! K kills, teams) are new fields of `EndRules` and new `RoundEnd` variants,
//! not a new type.

use bevy_ecs::entity::Entity;
use serde::{Deserialize, Serialize};

use crate::constants::TICK_HZ;
use crate::{MatchPhase, Score};

/// What happens to a rider after its death.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Respawn {
    /// Dead until the next round.
    #[default]
    Never,
    /// Back in the arena during the round: automatic riders this many ticks
    /// after the death, humans when they ask for it.
    After(u32),
}

/// Ticks a dead automatic rider (a bot) waits before it rides again, in a
/// round with respawns.
pub const RESPAWN_DELAY_TICKS: u32 = 240;

impl Respawn {
    /// Ticks between a death and an automatic respawn, `None` when riders
    /// stay dead until the next round.
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
    /// Round length in ticks; `None` for a round without a clock.
    #[serde(default)]
    pub time_limit: Option<u32>,
    /// End when at most one rider is alive; the survivor wins the round.
    #[serde(default)]
    pub last_standing: bool,
    /// End when no human is left in the round: all of them dead with no
    /// respawn. With respawns a dead human can come back, so it never fires.
    #[serde(default)]
    pub no_human_left: bool,
}

/// Why a round ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundEnd {
    /// At most one rider alive; with exactly one alive it is the winner.
    LastStanding,
    TimeLimit,
    /// Every human is out of the round; the riders left are not humans.
    NoHumanLeft,
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
    /// A match needs a human present: with none, it waits (`Free`) and is
    /// dropped when the last one leaves.
    #[serde(default)]
    pub needs_human: bool,
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
            needs_human: false,
        }
    }
}

impl MatchRules {
    /// Whether a round that began `elapsed` ticks ago is over, and why.
    pub fn round_end(&self, elapsed: u32, roster: &Roster) -> Option<RoundEnd> {
        if self.end.last_standing && roster.alive.len() <= 1 {
            return Some(RoundEnd::LastStanding);
        }
        if self.end.no_human_left && self.respawn == Respawn::Never && roster.humans_alive == 0 {
            return Some(RoundEnd::NoHumanLeft);
        }
        match self.end.time_limit {
            Some(limit) if elapsed >= limit => Some(RoundEnd::TimeLimit),
            _ => None,
        }
    }

    /// Riders a match needs to start: a last-standing round with one rider
    /// would be over on its first tick.
    pub fn min_riders(&self) -> usize {
        if self.end.last_standing { 2 } else { 1 }
    }

    /// Reject rules a round cannot be played by. A round with no end
    /// condition is allowed: it lasts until the match is dropped.
    pub fn validate(&self) -> Result<(), String> {
        if self.rounds == 0 {
            return Err("match: rounds must be at least 1".into());
        }
        if self.end.time_limit == Some(0) {
            return Err("match: time_limit must be at least 1 tick".into());
        }
        if self.end.last_standing && self.respawn != Respawn::Never {
            return Err("match: last_standing needs respawn = \"never\"".into());
        }
        let ends =
            self.end.time_limit.is_some() || self.end.last_standing || self.end.no_human_left;
        if self.respawn == Respawn::Never && !ends {
            // The dead would never come back and the round never end.
            return Err("match: a round without respawns needs an end condition".into());
        }
        Ok(())
    }
}

/// Who is in the arena on this tick, as the flow needs to see it. The caller
/// builds it from its own riders; the flow never sees how they joined.
#[derive(Clone, Copy, Debug)]
pub struct Roster<'a> {
    /// Everybody in the arena roster, alive or not.
    pub riders: usize,
    /// Humans among them.
    pub humans: usize,
    /// Riders still riding, in a stable order.
    pub alive: &'a [Entity],
    /// Humans among `alive`.
    pub humans_alive: usize,
}

impl<'a> Roster<'a> {
    /// A roster without humans (the lab's, and the flow's tests).
    pub fn riders(riders: usize, alive: &'a [Entity]) -> Self {
        Self {
            riders,
            humans: 0,
            alive,
            humans_alive: 0,
        }
    }
}

/// What [`MatchFlow`] decided for one tick; the caller applies it to its world.
#[derive(Debug, PartialEq, Eq)]
pub enum MatchStep {
    Idle,
    /// Enough riders are here: stop everybody, count down to round 1.
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
    /// Idle until enough riders are here, then a countdown and round 1.
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

    /// Whether a match is on. When false the match waits for riders.
    pub fn is_running(&self) -> bool {
        self.phase != MatchPhase::Free
    }

    /// Whether a dead rider may be placed now. While the match waits, humans
    /// ride free and nobody else rides, unless the match is about to start
    /// (it would retire them at once); during a round, respawns follow the
    /// rules; between rounds nobody is placed.
    pub fn may_spawn(&self, human: bool, roster: &Roster) -> bool {
        match self.phase {
            MatchPhase::Free => human && !self.can_start(roster),
            MatchPhase::Playing => self.rules.respawn != Respawn::Never,
            MatchPhase::Starting | MatchPhase::Intermission | MatchPhase::MatchOver => false,
        }
    }

    /// The start condition of a match: enough riders, and a human when the
    /// rules need one.
    fn can_start(&self, roster: &Roster) -> bool {
        roster.riders >= self.rules.min_riders() && (!self.rules.needs_human || roster.humans > 0)
    }

    /// Advance one fixed tick, after the tick's deaths.
    pub fn advance(&mut self, roster: &Roster) -> MatchStep {
        if self.rules.needs_human && roster.humans == 0 {
            // Nobody to play for: drop the match, or keep waiting.
            self.enter(MatchPhase::Free, 0);
            return MatchStep::Idle;
        }
        match self.phase {
            MatchPhase::Free => {
                if !self.can_start(roster) {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Starting, 1);
                MatchStep::Starting
            }
            MatchPhase::Starting => {
                if !self.wait(roster.riders, self.rules.intermission) {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Playing, 1);
                MatchStep::Begin { new_match: true }
            }
            MatchPhase::Playing => {
                self.ticks += 1;
                let Some(end) = self.rules.round_end(self.ticks, roster) else {
                    return MatchStep::Idle;
                };
                let winner = match (end, roster.alive) {
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
                if !self.wait(roster.riders, self.rules.intermission) {
                    return MatchStep::Idle;
                }
                self.enter(MatchPhase::Playing, self.round + 1);
                MatchStep::Begin { new_match: false }
            }
            MatchPhase::MatchOver => {
                if !self.wait(roster.riders, self.rules.final_intermission) {
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
        if riders < self.rules.min_riders() {
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

/// The winner of a round that ran out of time: most kills, then fewest
/// deaths. Without a kill nobody wins (a rider that never rode is not ahead of
/// those who died), and a tie at the top has no winner.
pub fn top_scorer(scores: &[(Entity, Score)]) -> Option<Entity> {
    let rank = |score: &Score| (score.kills, std::cmp::Reverse(score.deaths));
    let scorers: Vec<_> = scores.iter().filter(|(_, s)| s.kills > 0).collect();
    let best = scorers.iter().map(|(_, score)| rank(score)).max()?;
    match scorers
        .iter()
        .filter(|(_, s)| rank(s) == best)
        .collect::<Vec<_>>()[..]
    {
        [(entity, _)] => Some(*entity),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entities(n: u32) -> Vec<Entity> {
        (1..=n)
            .map(Entity::from_raw_u32)
            .map(Option::unwrap)
            .collect()
    }

    fn end(rules: &MatchRules, elapsed: u32, riders: &[Entity]) -> Option<RoundEnd> {
        rules.round_end(elapsed, &Roster::riders(riders.len(), riders))
    }

    #[test]
    fn last_standing_ends_on_one_or_none_alive() {
        let rules = MatchRules {
            end: EndRules {
                time_limit: Some(100),
                last_standing: true,
                ..EndRules::default()
            },
            ..MatchRules::default()
        };
        let riders = entities(3);
        assert_eq!(end(&rules, 10, &riders), None);
        assert_eq!(end(&rules, 10, &riders[..1]), Some(RoundEnd::LastStanding));
        assert_eq!(end(&rules, 10, &[]), Some(RoundEnd::LastStanding));
        assert_eq!(end(&rules, 100, &riders), Some(RoundEnd::TimeLimit));
    }

    #[test]
    fn time_limit_alone_ignores_deaths() {
        let rules = MatchRules {
            respawn: Respawn::After(240),
            end: EndRules {
                time_limit: Some(100),
                ..EndRules::default()
            },
            ..MatchRules::default()
        };
        assert_eq!(end(&rules, 99, &[]), None);
        assert_eq!(end(&rules, 100, &entities(4)), Some(RoundEnd::TimeLimit));
    }

    #[test]
    fn no_human_left_ends_a_round_without_respawns_only() {
        let mut rules = MatchRules {
            end: EndRules {
                no_human_left: true,
                ..EndRules::default()
            },
            ..MatchRules::default()
        };
        let bots = entities(3);
        let roster = |humans_alive| Roster {
            riders: 4,
            humans: 1,
            alive: &bots,
            humans_alive,
        };
        assert_eq!(rules.round_end(5, &roster(1)), None);
        assert_eq!(rules.round_end(5, &roster(0)), Some(RoundEnd::NoHumanLeft));
        // A dead human may come back: the round goes on.
        rules.respawn = Respawn::After(10);
        assert_eq!(rules.round_end(5, &roster(0)), None);
    }

    fn rounds_of(rounds: u32, limit: Option<u32>) -> MatchRules {
        MatchRules {
            respawn: Respawn::Never,
            end: EndRules {
                time_limit: limit,
                last_standing: true,
                ..EndRules::default()
            },
            rounds,
            intermission: 3,
            final_intermission: 5,
            needs_human: false,
        }
    }

    fn step(flow: &mut MatchFlow, riders: usize, alive: &[Entity]) -> MatchStep {
        flow.advance(&Roster::riders(riders, alive))
    }

    /// Run `ticks` idle ticks with everyone alive.
    fn idle(flow: &mut MatchFlow, riders: &[Entity], ticks: u32) {
        for _ in 0..ticks {
            assert_eq!(step(flow, riders.len(), riders), MatchStep::Idle);
        }
    }

    /// Bring a fresh match to the first tick of round 1.
    fn playing(flow: &mut MatchFlow, riders: &[Entity]) {
        assert_eq!(step(flow, riders.len(), riders), MatchStep::Starting);
        idle(flow, riders, 2);
        assert_eq!(
            step(flow, riders.len(), riders),
            MatchStep::Begin { new_match: true }
        );
        assert_eq!(flow.phase(), MatchPhase::Playing);
    }

    #[test]
    fn fewer_than_two_riders_keep_a_last_standing_match_free() {
        let mut flow = MatchFlow::new(rounds_of(2, None));
        let riders = entities(1);
        idle(&mut flow, &riders, 10);
        assert!(!flow.is_running());
        assert_eq!(step(&mut flow, 0, &[]), MatchStep::Idle);
    }

    #[test]
    fn a_second_rider_starts_a_countdown_then_round_one() {
        let mut flow = MatchFlow::new(rounds_of(2, None));
        playing(&mut flow, &entities(2));
        assert!(flow.is_running());
        assert_eq!(flow.round(), 1);
    }

    #[test]
    fn a_match_that_needs_a_human_waits_for_one_and_drops_when_none_is_left() {
        let rules = MatchRules {
            respawn: Respawn::After(10),
            end: EndRules {
                time_limit: Some(1000),
                ..EndRules::default()
            },
            intermission: 2,
            needs_human: true,
            ..MatchRules::default()
        };
        let mut flow = MatchFlow::new(rules);
        let riders = entities(3);
        // Bots alone never start it.
        idle(&mut flow, &riders, 5);
        assert!(!flow.is_running());
        // One human is enough without last standing.
        let with_human = Roster {
            riders: 4,
            humans: 1,
            alive: &riders,
            humans_alive: 0,
        };
        assert_eq!(flow.advance(&with_human), MatchStep::Starting);
        flow.advance(&with_human);
        assert_eq!(
            flow.advance(&with_human),
            MatchStep::Begin { new_match: true }
        );
        assert!(flow.may_spawn(false, &with_human));
        // The human leaves mid-round: the match is dropped, and the bots wait.
        let bots = Roster::riders(3, &riders);
        assert_eq!(flow.advance(&bots), MatchStep::Idle);
        assert!(!flow.is_running());
        assert!(!flow.may_spawn(false, &bots));
    }

    #[test]
    fn a_waiting_human_rides_free_until_the_match_can_start() {
        let rules = MatchRules {
            needs_human: true,
            ..rounds_of(3, None)
        };
        let flow = MatchFlow::new(rules);
        let alone = Roster {
            riders: 1,
            humans: 1,
            alive: &[],
            humans_alive: 0,
        };
        assert!(flow.may_spawn(true, &alone));
        assert!(!flow.may_spawn(false, &alone));
        // A second rider: the countdown starts this tick, nobody is placed.
        let pair = Roster { riders: 2, ..alone };
        assert!(!flow.may_spawn(true, &pair));
    }

    #[test]
    fn the_last_rider_alive_wins_and_the_break_precedes_the_next_round() {
        let mut flow = MatchFlow::new(rounds_of(3, None));
        let riders = entities(3);
        playing(&mut flow, &riders);
        assert!(!flow.may_spawn(true, &Roster::riders(3, &riders)));
        idle(&mut flow, &riders, 5);
        // Two of three die.
        assert_eq!(
            step(&mut flow, 3, &riders[..1]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: Some(riders[0])
            }
        );
        assert_eq!((flow.phase(), flow.round()), (MatchPhase::Intermission, 1));
        // The break lasts `intermission` ticks, then round 2 begins.
        idle(&mut flow, &riders, 2);
        assert_eq!(
            step(&mut flow, 3, &riders),
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
            step(&mut flow, 3, &[]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: None
            }
        );
        idle(&mut flow, &riders, 2);
        assert_eq!(
            step(&mut flow, 3, &riders),
            MatchStep::Begin { new_match: false }
        );
        // Time limit with several alive: round 2 lasts exactly 10 ticks.
        idle(&mut flow, &riders, 9);
        assert_eq!(
            step(&mut flow, 3, &riders),
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
            step(&mut flow, 2, &riders[1..]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: Some(riders[1])
            }
        );
        assert_eq!(flow.phase(), MatchPhase::MatchOver);
        idle(&mut flow, &riders, 4);
        assert_eq!(
            step(&mut flow, 2, &riders),
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
            step(&mut flow, 1, &riders[..1]),
            MatchStep::RoundOver {
                reason: RoundEnd::LastStanding,
                winner: Some(riders[0])
            }
        );
        // Still alone after the break started: back to free riding.
        assert_eq!(step(&mut flow, 1, &riders[..1]), MatchStep::Idle);
        assert!(!flow.is_running());
        assert_eq!(flow.round(), 0);
    }

    #[test]
    fn a_match_with_no_break_chains_its_rounds_from_round_one() {
        let rules = MatchRules {
            intermission: 0,
            end: EndRules {
                time_limit: Some(4),
                ..EndRules::default()
            },
            ..rounds_of(2, None)
        };
        let mut flow = MatchFlow::in_round(rules);
        // One rider is enough: a zero break never drops the match.
        let riders = entities(1);
        idle(&mut flow, &riders, 3);
        assert_eq!(
            step(&mut flow, 1, &riders),
            MatchStep::RoundOver {
                reason: RoundEnd::TimeLimit,
                winner: None
            }
        );
        assert_eq!(
            step(&mut flow, 1, &riders),
            MatchStep::Begin { new_match: false }
        );
        assert_eq!((flow.phase(), flow.round()), (MatchPhase::Playing, 2));
        idle(&mut flow, &riders, 3);
        assert_eq!(
            step(&mut flow, 1, &riders),
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
        step(&mut flow, 2, &riders);
        assert_eq!(flow.secs_left(), Some(1));
        flow.enter(MatchPhase::Free, 0);
        playing(&mut flow, &riders);
        assert_eq!(flow.secs_left(), Some(90));
        idle(&mut flow, &riders, secs_to_ticks(1));
        assert_eq!(flow.secs_left(), Some(89));
    }

    #[test]
    fn the_top_scorer_has_the_most_kills_then_the_fewest_deaths() {
        let [a, b, c] = entities(3)[..] else {
            unreachable!()
        };
        let score = |kills, deaths| Score {
            kills,
            deaths,
            wins: 0,
        };
        assert_eq!(
            top_scorer(&[(a, score(3, 5)), (b, score(4, 9)), (c, score(1, 0))]),
            Some(b)
        );
        assert_eq!(top_scorer(&[(a, score(3, 5)), (b, score(3, 2))]), Some(b));
        assert_eq!(top_scorer(&[(a, score(3, 2)), (b, score(3, 2))]), None);
        assert_eq!(top_scorer(&[]), None);
        // A late joiner at 0/0 is not ahead of riders who died without a kill.
        assert_eq!(top_scorer(&[(a, score(0, 1)), (b, score(0, 0))]), None);
    }
}
