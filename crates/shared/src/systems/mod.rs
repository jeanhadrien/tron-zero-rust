pub mod arena;
pub mod player;

use bevy_ecs::prelude::*;

/// System set ordering for the fixed simulation.
///
/// `Turn` runs before `Move` so a turn input is reflected in the same tick's
/// displacement (matches the JS Phase 1 → Phase 3 ordering).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    Turn,
    Move,
    Collision,
}
