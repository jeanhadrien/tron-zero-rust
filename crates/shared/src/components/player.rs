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
    super::trail::Trail,
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

/// Armed while alive; cleared when death cleanup has run.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct ShouldHandleDeath(pub bool);

/// Human life identity: a connection nonce in the high 64 bits and a life
/// counter in the low 64 bits. Prediction restores both during rollback.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct LifeGeneration(pub u128);

impl LifeGeneration {
    pub fn for_session(nonce: u64) -> Self {
        Self((nonce as u128) << 64)
    }

    pub fn next(self) -> Option<Self> {
        // Never let the life counter overflow into another connection nonce.
        ((self.0 as u64) != u64::MAX).then(|| Self(self.0 + 1))
    }
}

/// Client → server turn input (lightyear `Input` type).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum PlayerInput {
    #[default]
    None,
    TurnLeft,
    TurnRight,
    TurnLeftFor(u128),
    TurnRightFor(u128),
}

impl PlayerInput {
    pub fn for_life(self, generation: u128) -> Self {
        match self {
            Self::TurnLeft => Self::TurnLeftFor(generation),
            Self::TurnRight => Self::TurnRightFor(generation),
            _ => Self::None,
        }
    }

    /// Untagged inputs are for bots only; humans must match the current life.
    pub fn eligible_turn(self, life: Option<&LifeGeneration>) -> Self {
        match (self, life) {
            (Self::TurnLeftFor(generation), Some(life)) if generation == life.0 => Self::TurnLeft,
            (Self::TurnRightFor(generation), Some(life)) if generation == life.0 => Self::TurnRight,
            (Self::TurnLeft | Self::TurnRight, None) => self,
            _ => Self::None,
        }
    }
}

impl bevy_ecs::entity::MapEntities for PlayerInput {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}
