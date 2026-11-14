//! Grinding: the speed a line earns by riding close beside trails and walls.
//!
//! The sim accelerates a rider by the grind strength of the nearest obstacle on
//! each side (`shared::physics::grind_strength`) and lets the excess fade. Every
//! line in this game is axis-aligned, so the strength along a leg only comes from
//! obstacles that run parallel to it: they are found once per leg and the speed
//! is integrated along it with the sim's own constants. The rider's own trail
//! is left out by the caller: the sim does pay for it (from a few legs back),
//! but a bot that grinds its own coil only boxes itself in.

use glam::Vec2;
use shared::physics::{Segment, grind_strength};
use shared::{
    ARENA_WALL_GRIND, GRIND_ACCEL, GRIND_RANGE, MAX_SPEED_MULT, MIN_SPEED_MULT, SPEED_DECAY,
    SPEED_RECOVER, TURN_SPEED_FACTOR, rotate_left, rotate_right,
};

/// Distance between two samples of the strength along a leg.
const SAMPLE: f32 = 12.0;
/// Legs shorter than this carry no strength worth measuring.
const MIN_LEG: f32 = 1.0;

/// The speed a line reaches, as multiples of the base speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speed {
    /// Average over the ticks of the line.
    pub mean: f32,
    /// At its end.
    pub end: f32,
}

/// The speed of a rider that starts at `start` and rides `legs`, turning
/// between them (and before the first one when `turns_first`).
pub fn along(
    obstacles: &[Segment],
    legs: &[(Vec2, Vec2)],
    start: f32,
    step: f32,
    turns_first: bool,
) -> Speed {
    let mut c = if turns_first { turned(start) } else { start };
    let (mut weighted, mut ticks) = (0.0, 0.0);
    for (n, &(a, b)) in legs.iter().enumerate() {
        if n > 0 {
            c = turned(c);
        }
        let length = a.distance(b);
        if length < MIN_LEG {
            continue;
        }
        let dir = (b - a) / length;
        let sides = [rotate_left(dir), rotate_right(dir)]
            .map(|side| strips(obstacles, a, dir, side, length));
        let count = (length / SAMPLE).ceil().max(1.0) as usize;
        let dt = length / count as f32 / step;
        for k in 0..count {
            let at = (k as f32 + 0.5) * length / count as f32;
            let grind: f32 = sides.iter().map(|side| strength_at(side, at)).sum();
            let rate = if c < 1.0 { SPEED_RECOVER } else { SPEED_DECAY };
            c = (c + (GRIND_ACCEL * grind + (1.0 - c) * rate) * dt).min(MAX_SPEED_MULT);
            weighted += c * dt;
            ticks += dt;
        }
    }
    Speed {
        mean: if ticks > 0.0 { weighted / ticks } else { c },
        end: c,
    }
}

/// Cruise speed after one turn.
fn turned(c: f32) -> f32 {
    (c * TURN_SPEED_FACTOR).max(MIN_SPEED_MULT.min(c))
}

/// A stretch of a leg with a parallel obstacle beside it: `lo..hi` along the
/// leg, and the strength it pays there.
type Strip = (f32, f32, f32);

/// The parallel obstacles within grind range on the `side` of the leg from `a`
/// along `dir`, as strips of the leg.
fn strips(obstacles: &[Segment], a: Vec2, dir: Vec2, side: Vec2, length: f32) -> Vec<Strip> {
    let mut found = Vec::new();
    for seg in obstacles {
        let edge = seg.b - seg.a;
        if dir.perp_dot(edge).abs() > 1e-3 {
            continue;
        }
        let lateral = (seg.a - a).dot(side);
        if !(0.0..GRIND_RANGE).contains(&lateral) {
            continue;
        }
        let (t1, t2) = ((seg.a - a).dot(dir), (seg.b - a).dot(dir));
        let (lo, hi) = (t1.min(t2).max(0.0), t1.max(t2).min(length));
        if lo >= hi {
            continue;
        }
        let pays = if seg.owner.is_none() {
            ARENA_WALL_GRIND
        } else {
            1.0
        };
        found.push((lo, hi, grind_strength(lateral) * pays));
    }
    found
}

/// The strongest strip over `at` (the sim takes the nearest hit per side).
fn strength_at(strips: &[Strip], at: f32) -> f32 {
    strips
        .iter()
        .filter(|&&(lo, hi, _)| lo <= at && at <= hi)
        .map(|&(_, _, strength)| strength)
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trail(a: Vec2, b: Vec2) -> Segment {
        Segment {
            a,
            b,
            owner: Some(9),
            from_head: 10,
        }
    }

    #[test]
    fn riding_beside_a_trail_speeds_up_and_riding_free_does_not() {
        let obstacles = [trail(Vec2::new(0.0, 8.0), Vec2::new(900.0, 8.0))];
        let beside = [(Vec2::ZERO, Vec2::new(600.0, 0.0))];
        let far = [(Vec2::new(0.0, -300.0), Vec2::new(600.0, -300.0))];
        let near = along(&obstacles, &beside, 1.0, 3.0, false);
        let free = along(&obstacles, &far, 1.0, 3.0, false);
        assert!(near.end > 1.3, "{near:?}");
        assert!((free.end - 1.0).abs() < 1e-4, "{free:?}");
    }

    #[test]
    fn a_wall_pays_less_than_a_trail_and_a_turn_costs_speed() {
        let wall = Segment {
            owner: None,
            ..trail(Vec2::new(0.0, 8.0), Vec2::new(900.0, 8.0))
        };
        let beside = [(Vec2::ZERO, Vec2::new(600.0, 0.0))];
        let on_wall = along(&[wall], &beside, 1.0, 3.0, false);
        let on_trail = along(&[trail(wall.a, wall.b)], &beside, 1.0, 3.0, false);
        assert!(on_wall.end > 1.0 && on_wall.end < on_trail.end);
        let turned = along(&[], &beside, 1.0, 3.0, true);
        assert!(turned.mean < 1.0);
    }
}
