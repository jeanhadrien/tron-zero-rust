//! Arena entity components.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::constants::{ARENA_HEIGHT, ARENA_WIDTH};

/// Marker for the single arena entity.
#[derive(Component, Default, Serialize, Deserialize, Reflect)]
#[require(ArenaSize, WallSegments)]
pub struct Arena;

/// Arena dimensions in world units.
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
pub struct ArenaSize {
    pub width: f32,
    pub height: f32,
}

impl Default for ArenaSize {
    fn default() -> Self {
        Self {
            width: ARENA_WIDTH,
            height: ARENA_HEIGHT,
        }
    }
}

/// Boundary wall segments as `(x1, y1, x2, y2)` line segments.
#[derive(Component, Clone, Debug, Default, Serialize, Deserialize, Reflect)]
pub struct WallSegments(pub Vec<[f32; 4]>);
