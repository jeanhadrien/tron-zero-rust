//! Read-only world snapshot and coarse grid used by bot sensing.
//!
//! Built once per decision epoch. Walls are shared behind an `Arc`; each rider
//! keeps its own trail so a forecaster can fork cheaply.

use std::sync::{Arc, OnceLock};

use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemParam;
use glam::Vec2;
use shared::physics::Segment;
use shared::{
    ARENA_WIDTH, Arena, Direction, IsAlive, Player, Position, Rubber, SLOW_DOWN_DISTANCE,
    SpeedMult, TargetSpeedMult, Trail, WallSegments,
};

/// Grid resolution per axis. Cell size is always derived, never hardcoded.
pub const GRID_N: usize = 32;
pub const GRID_CELLS: usize = GRID_N * GRID_N;
/// Half the slowdown distance so diagonal ray leaks don't create phantom exits.
const GRID_CLEARANCE: f32 = SLOW_DOWN_DISTANCE * 0.5;

#[derive(Clone, Debug)]
pub struct RiderSnapshot {
    /// Entity bits; identity survives a forecast so self-exclusion still works.
    pub id: u64,
    pub pos: Vec2,
    pub dir: Vec2,
    /// Trail corners, as replicated: the head is `pos`.
    pub trail: Trail,
    pub alive: bool,
    pub rubber: f32,
    pub speed_mult: f32,
    pub target_speed_mult: f32,
}

#[derive(Clone, Debug)]
pub struct GridCache {
    blocked: Vec<u8>,
}

impl GridCache {
    pub fn cell_size() -> f32 {
        ARENA_WIDTH / GRID_N as f32
    }

    /// Rasterize trail/wall segments once; a cell is blocked if a segment
    /// passes within `GRID_CLEARANCE` of its centre.
    pub fn build(segments: &[(Vec2, Vec2)]) -> Self {
        let mut blocked = vec![0u8; GRID_CELLS];
        let cell = Self::cell_size();
        let half = ARENA_WIDTH * 0.5;
        for &(a, b) in segments {
            let min = (a.min(b) - Vec2::splat(GRID_CLEARANCE) + Vec2::splat(half)) / cell;
            let max = (a.max(b) + Vec2::splat(GRID_CLEARANCE) + Vec2::splat(half)) / cell;
            let x0 = min.x.floor().max(0.0) as usize;
            let y0 = min.y.floor().max(0.0) as usize;
            let x1 = (max.x.ceil().min(GRID_N as f32)) as usize;
            let y1 = (max.y.ceil().min(GRID_N as f32)) as usize;
            for gy in y0..y1 {
                for gx in x0..x1 {
                    if point_segment_distance(Self::cell_center(gx, gy), a, b) <= GRID_CLEARANCE {
                        blocked[gy * GRID_N + gx] = 1;
                    }
                }
            }
        }
        Self { blocked }
    }

    /// An all-free grid for snapshots whose reachability is not evaluated
    /// (forecast projections). Never rasterizes.
    pub fn unresolved() -> Self {
        Self {
            blocked: vec![0u8; GRID_CELLS],
        }
    }

    pub fn is_blocked(&self, gx: usize, gy: usize) -> bool {
        self.blocked[gy * GRID_N + gx] != 0
    }

    /// World position → grid cell, clamped to the arena.
    pub fn cell_of(pos: Vec2) -> (usize, usize) {
        let cell = Self::cell_size();
        let half = ARENA_WIDTH * 0.5;
        let local = (pos + Vec2::splat(half)) / cell;
        (
            (local.x.floor().max(0.0) as usize).min(GRID_N - 1),
            (local.y.floor().max(0.0) as usize).min(GRID_N - 1),
        )
    }

    fn cell_center(gx: usize, gy: usize) -> Vec2 {
        let cell = Self::cell_size();
        let half = ARENA_WIDTH * 0.5;
        Vec2::new(
            (gx as f32 + 0.5) * cell - half,
            (gy as f32 + 0.5) * cell - half,
        )
    }
}

#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    pub walls: Arc<Vec<[f32; 4]>>,
    pub riders: Vec<RiderSnapshot>,
    pub tick: u32,
    pub grid: GridCache,
    /// Lazy obstacle geometry; shared across sensors and projections of the
    /// same snapshot.
    obstacles: OnceLock<Vec<Segment>>,
}

impl WorldSnapshot {
    pub fn new(walls: Arc<Vec<[f32; 4]>>, riders: Vec<RiderSnapshot>, tick: u32) -> Self {
        let mut segments: Vec<(Vec2, Vec2)> = walls
            .iter()
            .map(|w| (Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])))
            .collect();
        let mut points = Vec::new();
        for rider in &riders {
            if rider.alive {
                rider.trail.points_into(rider.pos, &mut points);
                segments.extend(points.windows(2).map(|p| (p[0], p[1])));
            }
        }
        let grid = GridCache::build(&segments);
        Self {
            walls,
            riders,
            tick,
            grid,
            obstacles: OnceLock::new(),
        }
    }

    /// A projection result: real walls/riders, but the grid is left unresolved
    /// (no rasterization) because forecast consumers read rider state only.
    pub fn projected(walls: Arc<Vec<[f32; 4]>>, riders: Vec<RiderSnapshot>, tick: u32) -> Self {
        Self {
            walls,
            riders,
            tick,
            grid: GridCache::unresolved(),
            obstacles: OnceLock::new(),
        }
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.riders.iter().position(|r| r.id == id)
    }

    /// Obstacle segments in the exact form `shared::physics::nearest` expects.
    /// Rider segment owners use the snapshot's rider *index*, not entity bits —
    /// `step_riders` is likewise called with the index, so self-exclusion matches.
    pub fn obstacles(&self) -> &[Segment] {
        self.obstacles.get_or_init(|| {
            let mut segments = Vec::new();
            for w in self.walls.iter() {
                segments.push(Segment {
                    a: Vec2::new(w[0], w[1]),
                    b: Vec2::new(w[2], w[3]),
                    owner: None,
                    from_head: 0,
                });
            }
            let mut points = Vec::new();
            for (i, rider) in self.riders.iter().enumerate() {
                if rider.alive {
                    rider.trail.points_into(rider.pos, &mut points);
                    segments.extend(Segment::trail_legs(&points, i as u64));
                }
            }
            segments
        })
    }
}

/// Builds a snapshot from the live ECS. Reads `Position/Direction/Trail/IsAlive/
/// Rubber/SpeedMult/TargetSpeedMult` and `WallSegments`; the target multiplier
/// is required so forecasting matches the sim's step exactly.
#[derive(SystemParam)]
pub struct WorldSnapshotBuilder<'w, 's> {
    walls: Query<'w, 's, &'static WallSegments, With<Arena>>,
    #[allow(clippy::type_complexity)]
    riders: Query<
        'w,
        's,
        (
            Entity,
            &'static Position,
            &'static Direction,
            &'static Trail,
            &'static IsAlive,
            &'static Rubber,
            &'static SpeedMult,
            &'static TargetSpeedMult,
        ),
        With<Player>,
    >,
}

impl WorldSnapshotBuilder<'_, '_> {
    pub fn build(&self, tick: u32) -> WorldSnapshot {
        let walls = Arc::new(
            self.walls
                .iter()
                .next()
                .map(|w| w.0.clone())
                .unwrap_or_default(),
        );
        let riders = self
            .riders
            .iter()
            .map(
                |(entity, position, direction, trail, alive, rubber, speed, target)| {
                    RiderSnapshot {
                        id: entity.to_bits(),
                        pos: position.0,
                        dir: direction.0,
                        trail: trail.clone(),
                        alive: alive.0,
                        rubber: rubber.0,
                        speed_mult: speed.0,
                        target_speed_mult: target.0,
                    }
                },
            )
            .collect();
        WorldSnapshot::new(walls, riders, tick)
    }
}

fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let edge = b - a;
    let length_squared = edge.length_squared();
    if length_squared <= 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(edge) / length_squared).clamp(0.0, 1.0);
    p.distance(a + edge * t)
}
