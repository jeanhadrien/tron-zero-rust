//! Pattern score: how much of a rider's path is a repeated motif.
//!
//! A path is the sequence of its legs: a turn side (left or right) and the
//! length ridden before the next turn. A stretch of legs is *patterned* when,
//! for a period `p` of 1 to `MAX_PERIOD` legs, every leg has the side of the
//! leg `p` before and the difference of their lengths stays the same from leg
//! to leg (within a tolerance). A difference of zero is a repeated motif
//! (staircase, zigzag, square wave); a constant nonzero difference is a motif
//! that grows (square spiral). The stretch must run for two periods at least
//! (three when it grows, since two legs always form a progression).
//!
//! The score is the length of the patterned legs over the length of all legs.
//! It is a proxy for "draws something repeatable", nothing more: see
//! `docs/guides/dev.md`.

use crate::sim::{Event, EventKind};

/// Longest motif, in legs.
const MAX_PERIOD: usize = 4;
/// Two lengths match when they differ by at most this much (world units)
/// plus `LEN_TOL_REL` of the larger. Turns are tick-quantized (3 units).
const LEN_TOL_ABS: f32 = 15.0;
const LEN_TOL_REL: f32 = 0.1;

/// One leg of a path: the turn that starts it, and its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    /// `1` for a left turn, `-1` for a right turn.
    pub side: i8,
    pub len: f32,
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= LEN_TOL_ABS + LEN_TOL_REL * a.abs().max(b.abs())
}

/// Mark the legs covered by a finished run of matches `first..=last` of
/// period `p`, when long enough.
fn flush(covered: &mut [bool], p: usize, run: Option<(usize, usize, bool)>) {
    let Some((first, last, grows)) = run else {
        return;
    };
    let need = if grows { p.max(3) } else { p.max(2) };
    if last + 1 - first >= need {
        covered[first - p..=last].fill(true);
    }
}

/// Total length of the legs that belong to a repeated motif.
pub fn covered_length(legs: &[Leg]) -> f64 {
    let mut covered = vec![false; legs.len()];
    for p in 1..=MAX_PERIOD.min(legs.len()) {
        // Current run: first match, last match, whether the lengths drift.
        let mut run: Option<(usize, usize, bool)> = None;
        let mut prev_diff = 0.0;
        for i in p..legs.len() {
            let (leg, before) = (legs[i], legs[i - p]);
            if leg.side != before.side {
                flush(&mut covered, p, run.take());
                continue;
            }
            let diff = leg.len - before.len;
            let grows = !close(leg.len, before.len);
            let steady = (diff - prev_diff).abs() <= LEN_TOL_ABS + LEN_TOL_REL * leg.len;
            match &mut run {
                Some((_, last, drift)) if steady => {
                    *last = i;
                    *drift |= grows;
                }
                _ => {
                    flush(&mut covered, p, run.take());
                    run = Some((i, i, grows));
                }
            }
            prev_diff = diff;
        }
        flush(&mut covered, p, run);
    }
    legs.iter()
        .zip(covered)
        .filter(|(_, covered)| *covered)
        .map(|(leg, _)| f64::from(leg.len))
        .sum()
}

/// `covered_length` over the length of all legs; `0` for a path without legs.
pub fn score(legs: &[Leg]) -> f64 {
    let total: f64 = legs.iter().map(|leg| f64::from(leg.len)).sum();
    if total > 0.0 {
        covered_length(legs) / total
    } else {
        0.0
    }
}

fn heading(label: &str) -> [f64; 2] {
    match label {
        "+x" => [1.0, 0.0],
        "-x" => [-1.0, 0.0],
        "+y" => [0.0, 1.0],
        "-y" => [0.0, -1.0],
        other => panic!("not a cardinal heading: {other}"),
    }
}

/// The legs of one life from its turns (corner and the heading taken). The
/// first turn only sets the heading; the leg after the last turn is unfinished
/// and dropped.
pub fn legs(turns: &[([f64; 2], &str)]) -> Vec<Leg> {
    turns
        .windows(3)
        .map(|w| {
            let (before, at, next) = (heading(w[0].1), heading(w[1].1), w[2].0);
            let cross = before[0] * at[1] - before[1] * at[0];
            Leg {
                side: if cross > 0.0 { 1 } else { -1 },
                len: (next[0] - w[1].0[0]).hypot(next[1] - w[1].0[1]) as f32,
            }
        })
        .collect()
}

/// Patterned length per rider over a trace: the sum, over every life, of
/// `covered_length` of its legs. Lives end on a death, a respawn or a round
/// boundary. Riders are in `names`' order.
pub fn covered_by_rider(events: &[Event], names: &[String]) -> Vec<f64> {
    let mut turns: Vec<Vec<([f64; 2], &str)>> = vec![Vec::new(); names.len()];
    let mut covered = vec![0.0; names.len()];
    let end_life = |turns: &mut Vec<([f64; 2], &str)>, covered: &mut f64| {
        *covered += covered_length(&legs(turns));
        turns.clear();
    };
    for event in events {
        let rider = names.iter().position(|n| *n == event.rider);
        match (&event.kind, rider) {
            (EventKind::Turn { at, heading }, Some(r)) => turns[r].push((*at, heading)),
            (EventKind::Death { .. } | EventKind::Respawn { .. }, Some(r)) => {
                end_life(&mut turns[r], &mut covered[r]);
            }
            (EventKind::RoundStart { .. } | EventKind::RoundEnd { .. }, None) => {
                for (t, c) in turns.iter_mut().zip(&mut covered) {
                    end_life(t, c);
                }
            }
            _ => {}
        }
    }
    for (t, c) in turns.iter_mut().zip(&mut covered) {
        end_life(t, c);
    }
    covered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legs_of(sides_lens: &[(i8, f32)]) -> Vec<Leg> {
        sides_lens
            .iter()
            .map(|&(side, len)| Leg { side, len })
            .collect()
    }

    fn alternating(count: usize, len: impl Fn(usize) -> f32) -> Vec<Leg> {
        let path: Vec<(i8, f32)> = (0..count)
            .map(|i| (if i % 2 == 0 { 1 } else { -1 }, len(i)))
            .collect();
        legs_of(&path)
    }

    #[test]
    fn a_staircase_scores_one() {
        assert!(score(&alternating(12, |_| 300.0)) > 0.99);
    }

    #[test]
    fn a_staircase_with_tick_jitter_still_scores_high() {
        assert!(score(&alternating(12, |i| 300.0 + (i % 3) as f32 * 3.0)) > 0.99);
    }

    #[test]
    fn a_square_wave_scores_one() {
        // Left, right, right, left, left, ... with equal legs.
        let sides = [1, -1, -1, 1];
        let path: Vec<(i8, f32)> = (0..16).map(|i| (sides[i % 4], 250.0)).collect();
        assert!(score(&legs_of(&path)) > 0.99);
    }

    #[test]
    fn a_square_spiral_scores_one() {
        // Always left, legs growing by a constant step.
        let path: Vec<(i8, f32)> = (0..14).map(|i| (1, 100.0 + 60.0 * i as f32)).collect();
        assert!(score(&legs_of(&path)) > 0.99);
    }

    #[test]
    fn a_pattern_after_a_lead_in_scores_its_share() {
        let lead = [(1, 123.0), (1, 777.0), (-1, 410.0)];
        let mut path = legs_of(&lead);
        path.extend(alternating(10, |_| 200.0));
        let lead_len: f64 = lead.iter().map(|p| f64::from(p.1)).sum();
        let expected = 2000.0 / (2000.0 + lead_len);
        assert!((score(&path) - expected).abs() < 0.05);
    }

    #[test]
    fn straight_and_single_turns_score_zero() {
        assert_eq!(score(&[]), 0.0);
        assert_eq!(score(&legs_of(&[(1, 900.0)])), 0.0);
        assert_eq!(score(&legs_of(&[(1, 900.0), (-1, 400.0)])), 0.0);
    }

    #[test]
    fn a_random_walk_scores_near_zero() {
        // A fixed pseudo-random walk (LCG), so the test is deterministic.
        let mut state = 12345u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as f32 / (1u64 << 31) as f32
        };
        let path: Vec<(i8, f32)> = (0..300)
            .map(|_| (if next() < 0.5 { 1 } else { -1 }, 100.0 + 900.0 * next()))
            .collect();
        let s = score(&legs_of(&path));
        assert!(s < 0.2, "random walk scored {s}");
    }

    #[test]
    fn legs_come_from_turn_corners_and_headings() {
        // East, then north (left), east (right), north (left): legs of 200.
        let turns = [
            ([0.0, 0.0], "+x"),
            ([100.0, 0.0], "+y"),
            ([100.0, 200.0], "+x"),
            ([300.0, 200.0], "+y"),
        ];
        let legs = legs(&turns);
        assert_eq!(legs.len(), 2);
        assert_eq!((legs[0].side, legs[0].len), (1, 200.0));
        assert_eq!((legs[1].side, legs[1].len), (-1, 200.0));
    }
}
