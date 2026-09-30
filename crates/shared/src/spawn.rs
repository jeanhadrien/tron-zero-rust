//! Bounded authoritative spawn search against the current obstacle snapshot.

use glam::Vec2;

use crate::ArenaSize;

/// A rider (radius 14) starts well outside rubber/sliding range.
pub const SPAWN_CLEARANCE: f32 = 80.0;
/// One second of base-speed forward space, with clearance along the full path.
pub const SPAWN_FORWARD: f32 = crate::BASE_SPEED;
pub const SPAWN_WALL_MARGIN: f32 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SafeSpawn {
    pub position: Vec2,
    pub direction: Vec2,
}

fn point_distance_squared(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let edge = b - a;
    let t = if edge.length_squared() > 0.0 {
        ((p - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance_squared(a + edge * t)
}

fn segment_distance_squared(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f32 {
    let ab = b - a;
    let cd = d - c;
    let cross = ab.perp_dot(cd);
    if cross.abs() > f32::EPSILON {
        let t = (c - a).perp_dot(cd) / cross;
        let u = (c - a).perp_dot(ab) / cross;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return 0.0;
        }
    }
    point_distance_squared(a, c, d)
        .min(point_distance_squared(b, c, d))
        .min(point_distance_squared(c, a, b))
        .min(point_distance_squared(d, a, b))
}

pub fn spawn_is_safe(
    spawn: SafeSpawn,
    size: ArenaSize,
    segments: &[[Vec2; 2]],
    riders: &[Vec2],
) -> bool {
    let end = spawn.position + spawn.direction * SPAWN_FORWARD;
    let half = Vec2::new(size.width, size.height) * 0.5;
    if !half.is_finite()
        || !spawn.position.is_finite()
        || !spawn.direction.is_finite()
        || ![Vec2::X, Vec2::Y, Vec2::NEG_X, Vec2::NEG_Y].contains(&spawn.direction)
        || spawn
            .position
            .abs()
            .cmpgt(half - Vec2::splat(SPAWN_WALL_MARGIN))
            .any()
        || end.abs().cmpgt(half - Vec2::splat(SPAWN_CLEARANCE)).any()
    {
        return false;
    }
    let clearance_squared = SPAWN_CLEARANCE.powi(2);
    segments.iter().all(|[a, b]| {
        a.is_finite()
            && b.is_finite()
            && segment_distance_squared(spawn.position, end, *a, *b) >= clearance_squared
    }) && riders.iter().all(|p| {
        p.is_finite() && point_distance_squared(*p, spawn.position, end) >= clearance_squared
    })
}

/// Try seeded candidates, then a deterministic clearance-spaced grid. Failure
/// means no safe candidate was found, never permission to use an unsafe fallback.
pub fn find_safe_spawn(
    size: ArenaSize,
    segments: &[[Vec2; 2]],
    riders: &[Vec2],
    mut seed: u64,
) -> Option<SafeSpawn> {
    let extent = Vec2::new(size.width, size.height) - Vec2::splat(2.0 * SPAWN_WALL_MARGIN);
    if !extent.is_finite() || extent.min_element() < 0.0 {
        return None;
    }
    let directions = [Vec2::X, Vec2::Y, Vec2::NEG_X, Vec2::NEG_Y];
    let mut random = || {
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = seed;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((value ^ (value >> 31)) >> 40) as f32 / 16_777_216.0
    };
    for _ in 0..128 {
        let position = Vec2::new(random() - 0.5, random() - 0.5) * extent;
        let first = (random() * 4.0) as usize;
        for offset in 0..4 {
            let spawn = SafeSpawn {
                position,
                direction: directions[(first + offset) % 4],
            };
            if spawn_is_safe(spawn, size, segments, riders) {
                return Some(spawn);
            }
        }
    }
    let columns = (extent.x / SPAWN_CLEARANCE).ceil() as u32;
    let rows = (extent.y / SPAWN_CLEARANCE).ceil() as u32;
    for y in 0..=rows {
        for x in 0..=columns {
            let position = -extent * 0.5
                + Vec2::new(
                    (x as f32 * SPAWN_CLEARANCE).min(extent.x),
                    (y as f32 * SPAWN_CLEARANCE).min(extent.y),
                );
            for direction in directions {
                let spawn = SafeSpawn {
                    position,
                    direction,
                };
                if spawn_is_safe(spawn, size, segments, riders) {
                    return Some(spawn);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_corridor_rejects_walls_trails_and_riders() {
        let size = ArenaSize::default();
        let spawn = SafeSpawn {
            position: Vec2::ZERO,
            direction: Vec2::X,
        };
        assert!(spawn_is_safe(spawn, size, &[], &[]));
        for segment in [
            [Vec2::new(200.0, -10.0), Vec2::new(200.0, 10.0)],
            [Vec2::new(100.0, 79.0), Vec2::new(300.0, 79.0)],
            [Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0)],
        ] {
            assert!(!spawn_is_safe(spawn, size, &[segment], &[]));
        }
        assert!(!spawn_is_safe(spawn, size, &[], &[Vec2::new(200.0, 0.0)]));
        assert!(!spawn_is_safe(
            SafeSpawn {
                position: Vec2::new(1000.0, 0.0),
                ..spawn
            },
            size,
            &[],
            &[]
        ));
        assert!(spawn_is_safe(spawn, size, &[], &[Vec2::new(200.0, 81.0)]));
    }

    #[test]
    fn search_is_deterministic_and_has_no_unsafe_fallback() {
        let size = ArenaSize::default();
        let walls: Vec<_> = crate::arena_walls(size.width, size.height)
            .into_iter()
            .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
            .collect();
        let spawn = find_safe_spawn(size, &walls, &[Vec2::ZERO], 17).unwrap();
        assert_eq!(
            Some(spawn),
            find_safe_spawn(size, &walls, &[Vec2::ZERO], 17)
        );
        assert!(spawn_is_safe(spawn, size, &walls, &[Vec2::ZERO]));
        assert_eq!(
            find_safe_spawn(
                ArenaSize {
                    width: 150.0,
                    height: 150.0
                },
                &[],
                &[],
                1
            ),
            None
        );
        let blocked: Vec<_> = (-1200..=1200)
            .step_by(40)
            .map(|x| [Vec2::new(x as f32, -1200.0), Vec2::new(x as f32, 1200.0)])
            .collect();
        assert_eq!(find_safe_spawn(size, &blocked, &[], 1), None);
    }
}
