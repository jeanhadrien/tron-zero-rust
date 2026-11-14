//! Style: riding a figure. An artist rides legs of one length and turns in a
//! fixed cycle of sides, so its trail is a repeating figure (staircase,
//! square wave) the eye reads as a pattern.
//!
//! The figure is part of the build (`Weights::motif` picks the cycle,
//! `Weights::stride` the leg length); how much the bot cares is
//! `Weights::style`. The planner adds the turns that continue the figure to
//! its candidates and rates every candidate here: a turn that comes when the
//! leg has reached the stride, on the side the cycle asks for, rates `1`; a
//! turn at the wrong time or on the wrong side rates down to `-1`; riding on
//! while the leg is still short rates well. Survival is not decided here: the
//! planner only counts the rating of a line that has room.

use glam::Vec2;
use shared::rotate_left;

use crate::objective::Weights;
use crate::plan::{Action, Kind, Maneuver, Turn};
use crate::tuning::Artist;

const L: Action = Action::Left;
const R: Action = Action::Right;

/// The turn cycles, by `Weights::motif`: staircase, square wave. (A meander,
/// R R L L, was tried: its strands need room beside each other and it jams
/// against a wall.)
const CYCLES: [&[Action]; 2] = [&[R, L], &[L, R, R, L]];

/// The figure a build rides.
#[derive(Clone, Copy, Debug)]
pub struct Figure {
    cycle: &'static [Action],
    stride: f32,
    /// What the figure turned the other way costs in rating: a wall may ask
    /// for it, but the figure as laid is preferred. A leg `slack` units off
    /// the stride still counts as on it: turns are tick-quantized.
    rating: Artist,
}

/// Where the bot is in its figure.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Ridden since the last corner (units).
    leg: f32,
    /// Index in the cycle of the next turn.
    next: usize,
}

impl Figure {
    /// The figure of `weights`, `None` for a bot without style.
    pub fn of(weights: &Weights, rating: &Artist) -> Option<Self> {
        (weights.style > 0.0).then(|| Self {
            cycle: CYCLES[(weights.motif.round().max(0.0) as usize).min(CYCLES.len() - 1)],
            stride: weights.stride.max(30.0),
            rating: *rating,
        })
    }

    /// Where a rider with trail `points` (tail first, head last) stands in the
    /// cycle: the phase that agrees with most of its last turns.
    pub fn progress(&self, points: &[Vec2]) -> Progress {
        let n = self.cycle.len();
        let leg = match points {
            [.., corner, head] => corner.distance(*head),
            _ => 0.0,
        };
        let sides: Vec<Action> = points
            .windows(3)
            .rev()
            .take(n)
            .map(|w| {
                let before = (w[1] - w[0]).normalize_or_zero();
                let after = (w[2] - w[1]).normalize_or_zero();
                if rotate_left(before).dot(after) > 0.0 {
                    L
                } else {
                    R
                }
            })
            .collect();
        // `sides[i]` is the turn `i + 1` turns before the next one.
        let agreeing = |next: usize| {
            sides
                .iter()
                .enumerate()
                .filter(|&(i, &side)| self.cycle[(next + n - 1 - i % n) % n] == side)
                .count()
        };
        let mut next = 0;
        for phase in 1..n {
            if agreeing(phase) > agreeing(next) {
                next = phase;
            }
        }
        Progress { leg, next }
    }

    /// The turns that continue the figure: the next one when the leg reaches
    /// the stride (also turned the other way, for when a wall is in the way),
    /// and with the one after it.
    pub fn candidates(&self, progress: Progress, step: f32, horizon: u32) -> Vec<Maneuver> {
        let due = ((self.stride - progress.leg) / step).round().max(0.0) as u32;
        let each = (self.stride / step).round().max(1.0) as u32;
        let first = self.cycle[progress.next];
        let second = self.cycle[(progress.next + 1) % self.cycle.len()];
        let mut list = Vec::new();
        if due < horizon {
            for action in [first, flip(first)] {
                list.push(Maneuver::new(Kind::Turn, &[Turn { at: due, action }]));
            }
            if due + each < horizon {
                let kind = if first == second {
                    Kind::UTurn
                } else {
                    Kind::Dodge
                };
                list.push(Maneuver::new(
                    kind,
                    &[
                        Turn {
                            at: due,
                            action: first,
                        },
                        Turn {
                            at: due + each,
                            action: second,
                        },
                    ],
                ));
            }
        }
        list
    }

    /// How well `maneuver` continues the figure, `-1..=1`.
    pub fn rate(&self, progress: Progress, maneuver: &Maneuver, step: f32, horizon: u32) -> f32 {
        let turns = maneuver.turns();
        if turns.is_empty() {
            // Riding on: right while the leg is short, wrong once it is overdue.
            let end = progress.leg + horizon as f32 * step;
            let over = (end - self.stride - self.rating.slack).max(0.0);
            return if over == 0.0 {
                0.5
            } else {
                -(over / (0.5 * self.stride)).min(1.0)
            };
        }
        let as_laid = self.rate_turns(progress, turns, step, false);
        let mirrored = self.rate_turns(progress, turns, step, true);
        as_laid.max(mirrored - self.rating.mirror_cost)
    }

    /// The mean rating of `turns`, against the cycle or its mirror image.
    fn rate_turns(&self, progress: Progress, turns: &[Turn], step: f32, mirror: bool) -> f32 {
        let mut leg = progress.leg;
        let mut at = 0;
        let mut total = 0.0;
        for (j, turn) in turns.iter().enumerate() {
            leg += (turn.at - at) as f32 * step;
            at = turn.at;
            let wanted = self.cycle[(progress.next + j) % self.cycle.len()];
            let wanted = if mirror { flip(wanted) } else { wanted };
            let side = if turn.action == wanted { 1.0 } else { -1.0 };
            let miss =
                ((leg - self.stride).abs() - self.rating.slack).max(0.0) / (0.5 * self.stride);
            total += 0.5 * side + 0.5 * (1.0 - 2.0 * miss.min(1.0));
            leg = 0.0;
        }
        total / turns.len() as f32
    }
}

/// The turn on the other side.
fn flip(action: Action) -> Action {
    match action {
        Action::Left => Action::Right,
        Action::Right => Action::Left,
        Action::Hold => Action::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn figure() -> Figure {
        let weights = Weights {
            style: 1.0,
            motif: 0.0,
            ..Weights::BASE
        };
        let tuning = crate::roster::builtin_tuning().unwrap();
        Figure::of(&weights, &tuning.artist).unwrap()
    }

    fn turn(at: u32, action: Action) -> Maneuver {
        Maneuver::new(Kind::Turn, &[Turn { at, action }])
    }

    #[test]
    fn a_turn_on_time_and_on_the_cycle_beats_an_early_or_wrong_one() {
        let figure = figure();
        // Just started, facing east: the staircase begins with a right turn.
        let progress = figure.progress(&[Vec2::new(-100.0, 0.0)]);
        assert_eq!(progress.next, 0);
        let step = 3.0;
        let on_time = figure.rate(progress, &turn(67, R), step, 64);
        let wrong_side = figure.rate(progress, &turn(67, L), step, 64);
        let early = figure.rate(progress, &turn(10, R), step, 64);
        assert!(on_time > 0.9, "{on_time}");
        assert!(on_time > wrong_side && on_time > early);
        // Riding past the stride without turning is rated down.
        assert!(figure.rate(progress, &Maneuver::hold(), step, 100) < 0.0);
        // After a right turn (east to south) the cycle asks for a left one.
        let after_right = figure.progress(&[
            Vec2::new(-100.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, -10.0),
        ]);
        assert_eq!(after_right.next, 1);
    }
}
