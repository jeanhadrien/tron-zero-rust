//! Arc-length helpers on trail polylines. Render-only: the sim never calls
//! these, so ordinary `f32` math (sqrt, clamp) is fine here.

use glam::Vec2;

/// How far a replicated head may sit from a newer snapshot of its own trail
/// and still count as on it: the sim is bit-identical, so only float noise.
const SNAPSHOT_TOLERANCE: f32 = 0.1;

/// Total arc length of the polyline.
pub fn length(points: &[Vec2]) -> f32 {
    points.windows(2).map(|p| p[0].distance(p[1])).sum()
}

/// Arc length from `point` to the end of `points`, if `point` lies within
/// `tolerance` of the polyline. The closest segment wins (the later one on a
/// tie), so a point just before a corner is not read as sitting past it.
pub fn arc_to_end(points: &[Vec2], point: Vec2, tolerance: f32) -> Option<f32> {
    let mut after = 0.0;
    let mut best: Option<(f32, f32)> = None; // (distance, arc to end)
    for pair in points.windows(2).rev() {
        let (a, b) = (pair[0], pair[1]);
        let length = a.distance(b);
        let along = if length > 0.0 {
            ((point - a).dot(b - a) / (length * length)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let distance = (a + (b - a) * along).distance(point);
        if distance <= tolerance && best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, after + length * (1.0 - along)));
        }
        after += length;
    }
    best.map(|(_, arc)| arc)
}

/// Cut `distance` of arc length off the end of the polyline. Never removes the
/// first point, so a cut longer than the trail leaves its start.
pub fn truncate_back(points: &mut Vec<Vec2>, mut distance: f32) {
    while points.len() >= 2 && distance > 0.0 {
        let last = points[points.len() - 1];
        let before = points[points.len() - 2];
        let length = before.distance(last);
        if distance >= length {
            distance -= length;
            points.pop();
        } else {
            let end = points.len() - 1;
            points[end] = last + (before - last) * (distance / length);
            return;
        }
    }
}

/// Interpolate a trail between two snapshots: the head slides along `end` from
/// where `start`'s head was. When `start`'s head is not on `end` (death,
/// respawn, trimmed trail) it steps from `start` to `end` at `t = 1`, like the
/// other interpolated components, so nothing cuts across space.
pub fn interpolate_trail(start: &[Vec2], end: &[Vec2], t: f32) -> Vec<Vec2> {
    let travelled = start
        .last()
        .and_then(|head| arc_to_end(end, *head, SNAPSHOT_TOLERANCE));
    if t >= 1.0 {
        return end.to_vec();
    }
    let Some(behind) = travelled else {
        // Not a continuation (death, respawn): switch at t = 1 like `step`.
        return start.to_vec();
    };
    let mut result = end.to_vec();
    truncate_back(&mut result, behind * (1.0 - t));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
        ]
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        a.distance(b) < 1e-3
    }

    #[test]
    fn arc_to_end_measures_around_corners() {
        let arc = |x, y| arc_to_end(&corner(), Vec2::new(x, y), 0.5);
        assert!((arc(4.0, 0.0).unwrap() - 16.0).abs() < 1e-3);
        assert!((arc(10.0, 4.0).unwrap() - 6.0).abs() < 1e-3);
        assert_eq!(arc(4.0, 5.0), None);
    }

    #[test]
    fn truncate_back_walks_across_vertices() {
        let mut trail = corner();
        truncate_back(&mut trail, 14.0);
        assert_eq!(trail.len(), 2);
        assert!(near(trail[1], Vec2::new(6.0, 0.0)));
        truncate_back(&mut trail, 100.0);
        assert_eq!(trail, vec![Vec2::new(0.0, 0.0)]);
    }

    #[test]
    fn interpolation_slides_the_head_through_a_corner() {
        let start = vec![Vec2::new(0.0, 0.0), Vec2::new(8.0, 0.0)];
        let end = corner();
        // 12 units travelled; half of it undone leaves the head at (10, 4).
        let half = interpolate_trail(&start, &end, 0.5);
        assert!(near(*half.last().unwrap(), Vec2::new(10.0, 4.0)));
        assert_eq!(interpolate_trail(&start, &end, 1.0), end);
        assert!(near(
            *interpolate_trail(&start, &end, 0.0).last().unwrap(),
            start[1]
        ));
    }

    #[test]
    fn interpolation_steps_when_start_is_elsewhere() {
        let start = vec![Vec2::new(500.0, 500.0)];
        assert_eq!(interpolate_trail(&start, &corner(), 0.3), start);
        assert_eq!(interpolate_trail(&start, &corner(), 1.0), corner());
    }
}
