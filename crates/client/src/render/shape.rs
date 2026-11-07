//! Rider outlines as pure points, so the world renderer and the menu preview
//! share them.
//!
//! Outlines live in the heading frame (+X is where the rider faces, +Y its
//! left) at unit radius, closed implicitly (the last point joins the first).
//! They are centered on the head point by bounding box: a triangle's tip sticks
//! out in front rather than sitting on the head.

use std::f32::consts::FRAC_1_SQRT_2;

use bevy::math::Vec2;
use shared::RiderShape;

// Circle: a 24-gon at 15 degree steps, written out so no trig is needed.
const C1: f32 = 0.965_926; // cos 15
const S1: f32 = 0.258_819; // sin 15
const C2: f32 = 0.866_025; // cos 30
const HALF: f32 = 0.5; // sin 30 / cos 60
const C3: f32 = FRAC_1_SQRT_2; // cos 45

const CIRCLE: [Vec2; 24] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(C1, S1),
    Vec2::new(C2, HALF),
    Vec2::new(C3, C3),
    Vec2::new(HALF, C2),
    Vec2::new(S1, C1),
    Vec2::new(0.0, 1.0),
    Vec2::new(-S1, C1),
    Vec2::new(-HALF, C2),
    Vec2::new(-C3, C3),
    Vec2::new(-C2, HALF),
    Vec2::new(-C1, S1),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-C1, -S1),
    Vec2::new(-C2, -HALF),
    Vec2::new(-C3, -C3),
    Vec2::new(-HALF, -C2),
    Vec2::new(-S1, -C1),
    Vec2::new(0.0, -1.0),
    Vec2::new(S1, -C1),
    Vec2::new(HALF, -C2),
    Vec2::new(C3, -C3),
    Vec2::new(C2, -HALF),
    Vec2::new(C1, -S1),
];

const TRIANGLE: [Vec2; 3] = [
    Vec2::new(0.9, 0.0),
    Vec2::new(-0.9, 0.8),
    Vec2::new(-0.9, -0.8),
];

const SQUARE: [Vec2; 4] = [
    Vec2::new(0.85, 0.85),
    Vec2::new(-0.85, 0.85),
    Vec2::new(-0.85, -0.85),
    Vec2::new(0.85, -0.85),
];

const DIAMOND: [Vec2; 4] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.0, 0.75),
    Vec2::new(-1.0, 0.0),
    Vec2::new(0.0, -0.75),
];

const HEXAGON: [Vec2; 6] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(HALF, C2),
    Vec2::new(-HALF, C2),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-HALF, -C2),
    Vec2::new(HALF, -C2),
];

/// An arrowhead with a notch at the back.
const CHEVRON: [Vec2; 4] = [
    Vec2::new(0.9, 0.0),
    Vec2::new(-0.9, 0.9),
    Vec2::new(-0.4, 0.0),
    Vec2::new(-0.9, -0.9),
];

/// The outline of `shape` at unit radius in the heading frame.
pub fn outline(shape: RiderShape) -> &'static [Vec2] {
    match shape {
        RiderShape::Circle => &CIRCLE,
        RiderShape::Triangle => &TRIANGLE,
        RiderShape::Square => &SQUARE,
        RiderShape::Diamond => &DIAMOND,
        RiderShape::Hexagon => &HEXAGON,
        RiderShape::Chevron => &CHEVRON,
    }
}

/// The outline placed in the world (or any y-up plane): scaled by `radius`,
/// turned to face `heading` (a unit vector) and moved to `center`. Yields the
/// first point again at the end, ready for a line strip.
pub fn placed(
    shape: RiderShape,
    center: Vec2,
    heading: Vec2,
    radius: f32,
) -> impl Iterator<Item = Vec2> {
    let points = outline(shape);
    let left = heading.perp();
    points
        .iter()
        .chain(points.first())
        .map(move |p| center + (heading * p.x + left * p.y) * radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(shape: RiderShape) -> (Vec2, Vec2) {
        outline(shape)
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| {
                (lo.min(*p), hi.max(*p))
            })
    }

    #[test]
    fn outlines_are_sized_like_a_rider_and_centered_on_the_head() {
        for shape in RiderShape::ALL {
            let (lo, hi) = bounds(shape);
            let middle = (lo + hi) / 2.0;
            assert!(middle.abs().max_element() < 1e-3, "{shape:?} off-center");
            let extent = (hi - lo).max_element() / 2.0;
            assert!((0.8..=1.05).contains(&extent), "{shape:?} extent {extent}");
            for point in outline(shape) {
                assert!(point.length() <= 1.3, "{shape:?} sticks out");
            }
        }
    }

    #[test]
    fn outlines_are_mirror_symmetric_about_the_heading_axis() {
        for shape in RiderShape::ALL {
            for p in outline(shape) {
                let mirrored = Vec2::new(p.x, -p.y);
                let found = outline(shape).iter().any(|q| q.distance(mirrored) < 1e-4);
                assert!(found, "{shape:?} lacks the mirror of {p:?}");
            }
        }
    }

    #[test]
    fn placing_turns_the_outline_to_face_the_heading() {
        let center = Vec2::new(100.0, -50.0);
        for heading in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y] {
            let points: Vec<Vec2> = placed(RiderShape::Triangle, center, heading, 10.0).collect();
            // Closed: first point repeated; the tip (first point) is ahead.
            assert_eq!(points.len(), 4);
            assert_eq!(points[0], points[3]);
            let tip = points[0] - center;
            assert!(tip.normalize().distance(heading) < 1e-5);
            assert!((tip.length() - 9.0).abs() < 1e-4);
            // The base sits behind the head, symmetric about the heading axis.
            let base = (points[1] + points[2]) / 2.0 - center;
            assert!(base.dot(heading) < 0.0 && base.perp_dot(heading).abs() < 1e-4);
        }
    }

    #[test]
    fn left_stays_left_whatever_the_heading() {
        // Heading +Y: the heading frame's +Y (left) maps to world -X.
        let points: Vec<Vec2> = placed(RiderShape::Chevron, Vec2::ZERO, Vec2::Y, 1.0).collect();
        assert!(points[1].x < 0.0, "left wing is on world -X");
    }
}
