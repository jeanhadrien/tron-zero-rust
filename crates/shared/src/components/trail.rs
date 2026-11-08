//! Ordered, rollbackable trail corners on the player itself.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::constants::TRAIL_MAX_LENGTH;

/// The corners of a rider's trail, oldest first: an anchor, then every turn
/// vertex still (at least partly) inside the trail. Empty for a dead rider.
///
/// Only what a client cannot infer is stored, so the component changes on a
/// turn or when its oldest corner falls out of the trail, not every tick (it
/// is replicated): the head is the rider's `Position`, and the tail is the
/// point [`TRAIL_MAX_LENGTH`] back from the head along the corners. Use
/// [`Self::points`] for the drawn and collidable geometry.
#[derive(Component, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Trail(pub Vec<Vec2>);

impl Trail {
    pub fn new(position: Vec2) -> Self {
        Self(vec![position])
    }

    /// Record a turn vertex at `position`. A second turn at the same spot (a
    /// stopped rider) adds nothing.
    pub fn turn(&mut self, position: Vec2) {
        if self.0.last().is_some_and(|&last| last != position) {
            self.0.push(position);
        }
    }

    /// The full trail geometry with the rider at `head`: tail, live corners,
    /// head. Empty for an empty trail, at least two points otherwise. Leg `i`
    /// of the result is `len - 2 - i` legs back from the head.
    pub fn points(&self, head: Vec2) -> Vec<Vec2> {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        self.points_into(head, &mut out);
        out
    }

    /// [`Self::points`] into a reused buffer (cleared first).
    pub fn points_into(&self, head: Vec2, out: &mut Vec<Vec2>) {
        out.clear();
        let Some((first, tail)) = self.tail(head) else {
            return;
        };
        out.push(tail);
        out.extend_from_slice(&self.0[first + 1..]);
        out.push(head);
    }

    /// Length of the trail with the rider at `head`.
    pub fn length(&self, head: Vec2) -> f32 {
        crate::polyline::length(&self.points(head))
    }

    /// Drop the corners that lie wholly behind the tail. Never changes
    /// [`Self::points`]; call it once per tick with the rider's final head.
    pub fn prune(&mut self, head: Vec2) {
        if let Some((first, _)) = self.tail(head)
            && first > 0
        {
            self.0.drain(..first);
        }
    }

    /// The tail: the oldest corner index still in the trail and the point
    /// [`TRAIL_MAX_LENGTH`] back from `head`, or the anchor when the trail is
    /// shorter. Walks back from the head so the result does not depend on the
    /// corners behind the tail: pruning them is exact. `+ - * /` only.
    fn tail(&self, head: Vec2) -> Option<(usize, Vec2)> {
        let mut left = TRAIL_MAX_LENGTH;
        let mut end = head;
        for (i, &corner) in self.0.iter().enumerate().rev() {
            let length = end.distance(corner);
            if length >= left {
                return Some((i, end.lerp(corner, left / length)));
            }
            left -= length;
            end = corner;
        }
        self.0.first().map(|&anchor| (0, anchor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: f32 = TRAIL_MAX_LENGTH;

    fn near(points: &[Vec2], expected: &[Vec2]) -> bool {
        points.len() == expected.len()
            && points
                .iter()
                .zip(expected)
                .all(|(a, b)| a.distance(*b) < 1e-3)
    }

    #[test]
    fn a_short_trail_runs_from_the_anchor_to_the_head() {
        let trail = Trail(vec![Vec2::ZERO, Vec2::new(100.0, 0.0)]);
        let head = Vec2::new(100.0, 50.0);
        assert_eq!(
            trail.points(head),
            vec![Vec2::ZERO, Vec2::new(100.0, 0.0), head]
        );
        assert_eq!(Trail::new(head).points(head), vec![head, head]);
        assert!(Trail::default().points(head).is_empty());
    }

    #[test]
    fn a_straight_trail_keeps_the_full_length_behind_the_head() {
        let trail = Trail::new(Vec2::ZERO);
        let head = Vec2::new(MAX + 500.0, 0.0);
        assert!(near(&trail.points(head), &[Vec2::new(500.0, 0.0), head]));
        assert!((trail.length(head) - MAX).abs() < 1e-2);
    }

    #[test]
    fn the_tail_slides_along_the_oldest_leg_and_corners_behind_it_go() {
        // Anchor, a corner 100 up, then along x: while the tail is on the
        // first leg the anchor stays; once past the corner it goes.
        let mut trail = Trail(vec![Vec2::ZERO, Vec2::new(0.0, 100.0)]);
        let head = Vec2::new(MAX - 40.0, 100.0);
        let expected = [Vec2::new(0.0, 60.0), Vec2::new(0.0, 100.0), head];
        assert!(near(&trail.points(head), &expected));
        trail.prune(head);
        assert_eq!(trail.0, vec![Vec2::ZERO, Vec2::new(0.0, 100.0)]);

        let head = Vec2::new(MAX + 40.0, 100.0);
        let before = trail.points(head);
        trail.prune(head);
        assert_eq!(trail.0, vec![Vec2::new(0.0, 100.0)]);
        assert_eq!(trail.points(head), before);
        assert!(near(&before, &[Vec2::new(40.0, 100.0), head]));
    }

    #[test]
    fn pruning_never_changes_the_geometry() {
        // A zigzag far longer than the cap, at awkward float positions.
        let mut corners = vec![Vec2::new(0.3, 0.7)];
        for i in 1..200 {
            let last = corners[i - 1];
            let step = if i % 2 == 0 {
                Vec2::new(0.0, 37.1)
            } else {
                Vec2::new(23.9, 0.0)
            };
            corners.push(last + step);
        }
        let head = *corners.last().unwrap() + Vec2::new(0.0, 11.3);
        let mut trail = Trail(corners);
        let before = trail.points(head);
        trail.prune(head);
        assert!(trail.0.len() < 200);
        assert_eq!(trail.points(head), before);
        assert!((trail.length(head) - MAX).abs() < 0.1);
    }

    #[test]
    fn a_repeated_turn_spot_adds_one_corner() {
        let mut trail = Trail::new(Vec2::ZERO);
        trail.turn(Vec2::ZERO);
        assert_eq!(trail.0.len(), 1);
        trail.turn(Vec2::X);
        trail.turn(Vec2::X);
        assert_eq!(trail.0, vec![Vec2::ZERO, Vec2::X]);
        let mut empty = Trail::default();
        empty.turn(Vec2::X);
        assert!(empty.0.is_empty());
    }
}
