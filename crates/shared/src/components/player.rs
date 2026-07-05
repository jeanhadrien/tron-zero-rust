//! Player entity components + input enum.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::ActionState;
use crate::constants::BASE_RUBBER;

/// Marker for a player entity (the lightcycle).
#[derive(Component, Default, Serialize, Deserialize, Reflect)]
#[require(
    PlayerId,
    Position,
    Direction,
    Velocity,
    SpeedMult,
    TargetSpeedMult,
    IsAlive,
    PlayerColor,
    Rubber,
    IsSliding,
    IsColliding,
    ShouldHandleDeath,
    TrailPointCount,
    TrailPointNextOrder,
    ActionState<PlayerInput>
)]
pub struct Player;

/// Stable identity across respawns.
#[derive(Component, Clone, Debug, Default, Serialize, Deserialize, Reflect)]
pub struct PlayerId(pub String);

/// World position.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Position(pub Vec2);

/// Unit heading. Constrained to ±X / ±Y; turns are 90° component swaps.
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Direction(pub Vec2);

impl Default for Direction {
    fn default() -> Self {
        Self(Vec2::new(1.0, 0.0))
    }
}

/// Per-tick displacement × 1000 (Position advances by `Velocity / 1000` each
/// tick). Kept for parity with the JS codebase's fixed-point convention.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Velocity(pub Vec2);

/// Current speed multiplier (1.0 = base speed).
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct SpeedMult(pub f32);

impl SpeedMult {
    pub const fn base() -> Self {
        Self(1.0)
    }
}

/// Target speed multiplier the actual `SpeedMult` drifts toward (inertia).
/// Boosted while sliding; decays toward 1.0 in open space.
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Reflect)]
pub struct TargetSpeedMult(pub f32);

impl Default for TargetSpeedMult {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Packed RGB `(r<<16)|(g<<8)|b`.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, Reflect)]
pub struct PlayerColor(pub u32);

/// Alive flag. Toggled on death, the entity persists for respawn.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct IsAlive(pub bool);

/// Rubber meter. Clamped to `[0, BASE_RUBBER]`. Reaching 0 → death.
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Rubber(pub f32);

impl Default for Rubber {
    fn default() -> Self {
        Self(BASE_RUBBER)
    }
}

/// True while a side sensor ray is within `SLOW_DOWN_DISTANCE` of an obstacle
/// (sliding/grinding). Triggers acceleration.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct IsSliding(pub bool);

/// True while the front sensor ray is within `SLOW_DOWN_DISTANCE` of an
/// obstacle (rubber zone engaged).
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct IsColliding(pub bool);

/// Guards one-shot death handling next Phase 1.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct ShouldHandleDeath(pub bool);

/// Mirrors the player's child `TrailPoint` count for cheap zero-checks.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct TrailPointCount(pub u32);

/// Monotonic counter for the next `TrailPointOrder` value. Lives on the
/// player so it rolls back with the player under lightyear prediction.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct TrailPointNextOrder(pub u32);

/// Client → server turn input (lightyear `Input` type).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum PlayerInput {
    #[default]
    None,
    TurnLeft,
    TurnRight,
}

impl bevy_ecs::entity::MapEntities for PlayerInput {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}
