//! Trail point child-entity components.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use super::player::{Direction, Position};

/// Marker for a trail point — a static vertex left at each turn site.
/// Child of the `Player` entity that spawned it.
#[derive(Component, Default, Serialize, Deserialize, Reflect)]
#[require(Position, Direction, TrailPointOrder)]
pub struct TrailPoint;

/// Sort key for trail rendering. Assigned monotonically by the parent
/// player's `TrailPointNextOrder` counter.
#[derive(Component, Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct TrailPointOrder(pub u32);
