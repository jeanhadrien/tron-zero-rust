//! Fog-of-war visibility (`docs/specs/fog.md`): what a viewer sees through a
//! cone, with trails and walls blocking the sight. Pure and deterministic, and
//! not part of the sim (only render code calls it, so it may use trig).

use std::f32::consts::{PI, TAU};

use glam::Vec2;

use crate::GRIND_RANGE;
use crate::physics::{Segment, ray_hit};

/// Proximity rule: radius of the circle around the rider that is always seen.
pub const FOG_PROXIMITY_RADIUS: f32 = 90.0;
/// Glow rule: radius around every seen trail part. Equal to `GRIND_RANGE`, so
/// anyone close enough to grind a seen trail is inside its glow.
pub const FOG_GLOW_RADIUS: f32 = GRIND_RANGE;
/// Sparks rule: radius of the circle around a grinding rider.
pub const FOG_SPARKS_RADIUS: f32 = 150.0;
/// Sparks rule: seconds the circle lasts after the grind ends.
pub const FOG_SPARKS_SECS: f32 = 2.0;

/// Fixed rays along the cone's arc, in radians (~2 degrees).
const ARC_STEP: f32 = 0.035;
/// Offset of the two rays that graze a segment end, in radians.
const GRAZE: f32 = 1e-4;
/// Hits closer than this are the viewer standing on its own trail: no block.
const EYE_GAP: f32 = 0.01;

/// One corner of the visibility polygon, which is a fan around the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub point: Vec2,
    /// Index (in the `segments` slice given) of the segment this ray hit;
    /// `None` when it reached the range (a point of the arc).
    pub hit: Option<usize>,
}

/// The cone's visibility polygon around `eye`, as a triangle fan: vertices
/// sorted by angle, from the cone's right edge to its left, the eye being the
/// fan's centre and not listed. `heading` points where the cone looks,
/// `cone_angle` is its full width in degrees. Every segment blocks, the
/// viewer's own trail included.
///
/// shortcut: O(rays x segments), switch to a sorted angular sweep if
/// `just profile client` shows it.
pub fn visibility_polygon(
    eye: Vec2,
    heading: Vec2,
    cone_angle: f32,
    range: f32,
    segments: &[Segment],
) -> Vec<Vertex> {
    let half = cone_angle.to_radians() * 0.5;
    let base = heading.to_angle();
    // Broad phase: segments whose box meets the box of the range circle.
    let (lo, hi) = (eye - Vec2::splat(range), eye + Vec2::splat(range));
    let near: Vec<usize> = (0..segments.len())
        .filter(|&i| {
            let s = &segments[i];
            s.a.max(s.b).cmpge(lo).all() && s.a.min(s.b).cmple(hi).all()
        })
        .collect();

    // Ray angles relative to the heading: the cone's edges, the arc, and a
    // ray at each segment end with one a hair to each side.
    let mut angles = vec![-half, half];
    let steps = (2.0 * half / ARC_STEP).ceil() as usize;
    angles.extend((1..steps).map(|i| -half + i as f32 * 2.0 * half / steps as f32));
    for &i in &near {
        for end in [segments[i].a, segments[i].b] {
            let rel = (end - eye).to_angle() - base;
            let rel = (rel + PI).rem_euclid(TAU) - PI;
            angles.extend(
                [rel - GRAZE, rel, rel + GRAZE]
                    .into_iter()
                    .filter(|a| a.abs() <= half),
            );
        }
    }
    angles.sort_by(f32::total_cmp);

    angles
        .into_iter()
        .map(|angle| {
            let dir = Vec2::from_angle(base + angle);
            let nearest = near
                .iter()
                .filter_map(|&i| ray_hit(eye, dir, segments[i].a, segments[i].b).map(|d| (d, i)))
                .filter(|&(d, _)| d > EYE_GAP && d < range)
                .min_by(|x, y| x.0.total_cmp(&y.0));
            match nearest {
                Some((d, i)) => Vertex {
                    point: eye + dir * d,
                    hit: Some(i),
                },
                None => Vertex {
                    point: eye + dir * range,
                    hit: None,
                },
            }
        })
        .collect()
}

/// The seen part of each segment: `(segment index, from, to)` for every two
/// neighbouring vertices on the same segment. A segment can come out in
/// several pieces (and with tiny ones at its ends); a caller that wants one
/// span per segment takes the hull.
pub fn visible_parts(polygon: &[Vertex]) -> impl Iterator<Item = (usize, Vec2, Vec2)> + '_ {
    polygon
        .windows(2)
        .filter_map(|w| match (w[0].hit, w[1].hit) {
            (Some(a), Some(b)) if a == b => Some((a, w[0].point, w[1].point)),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(a: Vec2, b: Vec2) -> Segment {
        Segment {
            a,
            b,
            owner: Some(1),
            from_head: 5,
        }
    }

    /// Angle of a vertex from the eye at the origin, in degrees.
    fn deg(v: &Vertex) -> f32 {
        v.point.to_angle().to_degrees()
    }

    #[test]
    fn open_field_is_the_full_cone() {
        let poly = visibility_polygon(Vec2::ZERO, Vec2::Y, 90.0, 100.0, &[]);
        assert!(poly.iter().all(|v| v.hit.is_none()));
        assert!(poly.iter().all(|v| (v.point.length() - 100.0).abs() < 1e-3));
        // Heading +Y is 90 degrees: the cone runs from 45 to 135.
        assert!((deg(&poly[0]) - 45.0).abs() < 0.01);
        assert!((deg(poly.last().unwrap()) - 135.0).abs() < 0.01);
        assert!(poly.windows(2).all(|w| deg(&w[0]) <= deg(&w[1])));
    }

    #[test]
    fn a_trail_blocks_what_is_behind_it() {
        // A vertical trail at x = 50 covers the whole 90 degree cone.
        let trail = [wall(Vec2::new(50.0, -100.0), Vec2::new(50.0, 100.0))];
        let poly = visibility_polygon(Vec2::ZERO, Vec2::X, 90.0, 300.0, &trail);
        assert!(poly.iter().all(|v| v.hit == Some(0)));
        assert!(poly.iter().all(|v| (v.point.x - 50.0).abs() < 1e-2));
    }

    #[test]
    fn what_is_outside_the_cone_or_range_is_not_seen() {
        let beside = wall(Vec2::new(0.0, 50.0), Vec2::new(0.0, 90.0));
        let far = wall(Vec2::new(500.0, -10.0), Vec2::new(500.0, 10.0));
        let poly = visibility_polygon(Vec2::ZERO, Vec2::X, 90.0, 300.0, &[beside, far]);
        assert!(poly.iter().all(|v| v.hit.is_none()));
    }

    #[test]
    fn the_viewers_own_trail_blocks_but_not_where_it_stands() {
        // The head leg ends on the eye; an older leg crosses the way ahead.
        let head_leg = wall(Vec2::new(-50.0, 0.0), Vec2::ZERO);
        let old_leg = wall(Vec2::new(100.0, -200.0), Vec2::new(100.0, 200.0));
        let poly = visibility_polygon(Vec2::ZERO, Vec2::X, 90.0, 300.0, &[head_leg, old_leg]);
        assert!(poly.iter().all(|v| v.hit == Some(1)));
    }

    #[test]
    fn visible_parts_span_the_seen_stretch_of_a_segment() {
        // The 90 degree cone sees x = 50 from y = -50 to 50, of a segment
        // that runs from -100 to 100.
        let trail = [wall(Vec2::new(50.0, -100.0), Vec2::new(50.0, 100.0))];
        let poly = visibility_polygon(Vec2::ZERO, Vec2::X, 90.0, 300.0, &trail);
        let parts: Vec<_> = visible_parts(&poly).collect();
        assert!(parts.iter().all(|p| p.0 == 0));
        let seen: f32 = parts.iter().map(|p| p.1.distance(p.2)).sum();
        assert!((seen - 100.0).abs() < 0.1, "{seen}");
    }
}
