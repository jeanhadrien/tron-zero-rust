//! Perception: an 8-ray continuous fan plus a capped BFS over the grid cache.
//!
//! Rays are cast against the exact polyline segments (never the grid); the grid
//! only supplies coarse reachable-area and exit counts.

use std::collections::VecDeque;

use glam::Vec2;
use shared::physics::nearest;
use shared::{rotate_left, rotate_right};

use crate::world::{GRID_CELLS, GRID_N, WorldSnapshot};

/// Full-circle sensor fan: every bot always casts all rays.
pub const RAY_COUNT: usize = 8;
/// Dead-ahead ray (forward semantics in `nearest`).
pub const RAY_FRONT: usize = 0;
/// 90° counter-clockwise ray.
pub const RAY_LEFT: usize = 1;
/// 90° clockwise ray.
pub const RAY_RIGHT: usize = 2;
/// Rear ray (`-dir`). Sensed/replicated/visualized only: no scoring weight
/// reads it, and it normally sees the rider's own older trail behind.
pub const RAY_REAR: usize = 3;
/// Front-left diagonal (45° between front and left).
pub const RAY_FRONT_LEFT: usize = 4;
/// Front-right diagonal (45° between front and right).
pub const RAY_FRONT_RIGHT: usize = 5;

/// Distance at which an obstacle stops reducing a ray's clearance.
pub const SENSE_HORIZON: f32 = 240.0;
const THREAT_RANGE: f32 = 600.0;

#[derive(Clone, Copy, Debug)]
pub struct Perception {
    pub rays: [f32; RAY_COUNT],
    pub freedom: u32,
    pub exits: u8,
    pub threat: f32,
    pub memory_trend: f32,
    /// Duelist's enemy turn-rate estimate, fed from `OpponentHistory` after
    /// sensing (keeps the `Strategy::score` signature stable).
    pub opponent_turn_prob: f32,
    pub tick: u32,
    /// Index into the snapshot's riders; lets strategies identify self.
    pub rider: usize,
}

impl Default for Perception {
    fn default() -> Self {
        Self {
            rays: [f32::INFINITY; RAY_COUNT],
            freedom: 1,
            exits: 0,
            threat: 0.0,
            memory_trend: 0.0,
            opponent_turn_prob: 0.0,
            tick: 0,
            rider: 0,
        }
    }
}

impl Perception {
    pub fn clearance_with(&self, index: usize, horizon: f32) -> f32 {
        let ray = self.rays[index];
        if ray.is_finite() && horizon > 0.0 {
            (ray / horizon).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Fraction of the arena reachable within the BFS budget, so both a fully
    /// open arena and a sealed pocket map onto `[0, 1]` meaningfully.
    pub fn freedom_ratio(&self) -> f32 {
        (self.freedom as f32 / GRID_CELLS as f32).clamp(0.0, 1.0)
    }

    pub fn exits_ratio(&self) -> f32 {
        self.exits as f32 / 4.0
    }
}

pub trait Sensor {
    fn sense(&self, world: &WorldSnapshot) -> Perception;
}

/// The default sensor: the full 8-ray fan plus a BFS capped at
/// `bfs_budget` nodes.
pub struct RaySensor {
    /// Index into the snapshot's riders.
    pub rider: usize,
    /// Capped BFS node budget.
    pub bfs_budget: u32,
}

impl Sensor for RaySensor {
    fn sense(&self, world: &WorldSnapshot) -> Perception {
        let Some(rider) = world.riders.get(self.rider) else {
            return Perception::default();
        };
        let obstacles = world.obstacles();
        let dir = rider.dir;
        let left = rotate_left(dir);
        let right = rotate_right(dir);
        let directions = [
            dir,
            left,
            right,
            // Rear ray: sensed/replicated/visualized only; normally sees
            // the rider's own older trail behind.
            -dir,
            (dir + left).normalize_or_zero(),
            (dir + right).normalize_or_zero(),
            (left - dir).normalize_or_zero(),
            (right - dir).normalize_or_zero(),
        ];
        // The rear ray mirrors `-dir` at its named index; scoring never reads
        // it, but the full fan is always cast and replicated.
        debug_assert!((directions[RAY_REAR] + dir).length_squared() < f32::EPSILON);
        let mut rays = [f32::INFINITY; RAY_COUNT];
        for (index, direction) in directions.iter().enumerate() {
            // Mirror `simulate_players`: only the front sensor uses forward
            // semantics; every other ray (including rear) excludes the
            // rider's own active edge and departure corner.
            let side = index != RAY_FRONT;
            rays[index] = nearest(
                obstacles,
                Some(self.rider as u64),
                rider.pos,
                *direction,
                side,
            );
        }
        let (freedom, exits) = measure_freedom(world, rider.pos, self.bfs_budget);
        Perception {
            rays,
            freedom,
            exits,
            threat: threat_level(world, self.rider),
            memory_trend: 0.0,
            opponent_turn_prob: 0.0,
            tick: world.tick,
            rider: self.rider,
        }
    }
}

/// Capped 4-neighbour BFS from the rider's cell. The start cell is always
/// reachable even if its own active trail segment rasterized onto it.
fn measure_freedom(world: &WorldSnapshot, pos: Vec2, budget: u32) -> (u32, u8) {
    let (sx, sy) = crate::world::GridCache::cell_of(pos);
    let mut visited = vec![false; GRID_N * GRID_N];
    let mut queue = VecDeque::new();
    visited[sy * GRID_N + sx] = true;
    queue.push_back((sx, sy));
    let mut exits = 0u8;
    for (nx, ny) in neighbors(sx, sy) {
        if !world.grid.is_blocked(nx, ny) {
            exits += 1;
        }
    }
    let cap = budget.max(1);
    let mut count = 1u32;
    while let Some((x, y)) = queue.pop_front() {
        if count >= cap {
            break;
        }
        for (nx, ny) in neighbors(x, y) {
            let index = ny * GRID_N + nx;
            if visited[index] || world.grid.is_blocked(nx, ny) {
                continue;
            }
            visited[index] = true;
            count += 1;
            queue.push_back((nx, ny));
        }
    }
    (count, exits)
}

fn neighbors(gx: usize, gy: usize) -> impl Iterator<Item = (usize, usize)> {
    [
        gx.checked_sub(1).map(|x| (x, gy)),
        (gx + 1 < GRID_N).then_some((gx + 1, gy)),
        gy.checked_sub(1).map(|y| (gx, y)),
        (gy + 1 < GRID_N).then_some((gx, gy + 1)),
    ]
    .into_iter()
    .flatten()
}

/// Stub proximity threat in `[0, 1]`: 1 when an enemy is on top of us.
fn threat_level(world: &WorldSnapshot, rider: usize) -> f32 {
    let Some(me) = world.riders.get(rider) else {
        return 0.0;
    };
    let nearest = world
        .riders
        .iter()
        .enumerate()
        .filter(|(i, other)| *i != rider && other.alive)
        .map(|(_, other)| me.pos.distance(other.pos));
    match nearest.reduce(f32::min) {
        Some(distance) => (1.0 - distance / THREAT_RANGE).clamp(0.0, 1.0),
        None => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::world::RiderSnapshot;

    fn rider(id: u64, pos: Vec2, dir: Vec2) -> RiderSnapshot {
        RiderSnapshot {
            id,
            pos,
            dir,
            // Short active edge only, so the rear ray is not blocked by our
            // own trail and a wall placed behind is visible.
            trail: vec![pos - dir * 10.0, pos],
            alive: true,
            rubber: 0.0,
            speed_mult: 1.0,
            target_speed_mult: 1.0,
        }
    }

    #[test]
    fn fan_order_matches_named_constants() {
        assert_eq!(RAY_COUNT, 8);
        assert_eq!(RAY_FRONT, 0);
        assert_eq!(RAY_LEFT, 1);
        assert_eq!(RAY_RIGHT, 2);
        assert_eq!(RAY_REAR, 3);
        assert_eq!(RAY_FRONT_LEFT, 4);
        assert_eq!(RAY_FRONT_RIGHT, 5);
    }

    #[test]
    fn rear_ray_is_cast_and_sees_obstacle_behind() {
        // Vertical wall at x = -50; rider at the origin facing +X.
        let walls = Arc::new(vec![[-50.0, -100.0, -50.0, 100.0]]);
        let world = WorldSnapshot::new(walls, vec![rider(7, Vec2::ZERO, Vec2::X)], 1);
        let perception = RaySensor {
            rider: 0,
            bfs_budget: 64,
        }
        .sense(&world);
        assert!(
            perception.rays[RAY_REAR].is_finite(),
            "rear ray must be cast: {:?}",
            perception.rays
        );
        assert!(
            (perception.rays[RAY_REAR] - 50.0).abs() < 1.0,
            "rear ray should see the wall behind: {:?}",
            perception.rays
        );
        assert!(
            perception.rays[RAY_FRONT].is_infinite(),
            "no wall ahead: {:?}",
            perception.rays
        );
    }
}
