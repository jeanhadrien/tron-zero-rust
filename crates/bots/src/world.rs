//! Read-only world snapshot the bots plan over.
//!
//! Built once per tick in which any bot looks. Walls are shared behind an
//! `Arc`; each rider keeps its own trail. The obstacle segments and the room
//! grid are built lazily and shared by every bot that looks at the snapshot.

use std::sync::{Arc, OnceLock};

use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemParam;
use glam::Vec2;
use shared::physics::Segment;
use shared::{
    Arena, Direction, IsAlive, Player, Position, Rubber, SpeedMult, TargetSpeedMult, Trail,
    WallSegments,
};

use crate::space::Space;

#[derive(Clone, Debug)]
pub struct RiderSnapshot {
    /// Entity bits.
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
pub struct WorldSnapshot {
    pub walls: Arc<Vec<[f32; 4]>>,
    pub riders: Vec<RiderSnapshot>,
    pub tick: u32,
    obstacles: OnceLock<Vec<Segment>>,
    space: OnceLock<Space>,
}

impl WorldSnapshot {
    pub fn new(walls: Arc<Vec<[f32; 4]>>, riders: Vec<RiderSnapshot>, tick: u32) -> Self {
        Self {
            walls,
            riders,
            tick,
            obstacles: OnceLock::new(),
            space: OnceLock::new(),
        }
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.riders.iter().position(|r| r.id == id)
    }

    /// Obstacle segments in the exact form `shared::physics::nearest` expects.
    /// Rider segment owners use the snapshot's rider *index*, not entity bits:
    /// `step_riders` is likewise called with the index, so self-exclusion
    /// matches.
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

    /// The room grid: every wall and every live trail.
    pub fn space(&self) -> &Space {
        self.space.get_or_init(|| {
            let trails: Vec<Vec<Vec2>> = self
                .riders
                .iter()
                .filter(|r| r.alive)
                .map(|r| r.trail.points(r.pos))
                .collect();
            Space::build(&self.walls, &trails)
        })
    }
}

/// Builds a snapshot from the live ECS. Reads `Position/Direction/Trail/IsAlive/
/// Rubber/SpeedMult/TargetSpeedMult` and `WallSegments`; the target multiplier
/// is required so forecasting matches the sim's step exactly.
#[derive(SystemParam)]
pub struct WorldSnapshotBuilder<'w, 's> {
    walls: Query<'w, 's, &'static WallSegments, With<Arena>>,
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
