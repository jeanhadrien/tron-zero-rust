//! Arena + player setup systems.

use bevy_ecs::prelude::*;
use glam::Vec2;

use crate::components::arena::{Arena, ArenaSize, WallSegments};
use crate::components::player::{
    Direction, IsAlive, Player, PlayerColor, PlayerId, Position, SpeedMult, TrailPointCount,
    TrailPointNextOrder, Velocity,
};
use crate::components::trail::{TrailPoint, TrailPointOrder};
use crate::math::arena_walls;

/// Spawn the arena entity. Runs once on startup.
pub fn setup_arena(mut commands: Commands) {
    let size = ArenaSize::default();
    commands.spawn((
        Arena,
        size,
        WallSegments(arena_walls(size.width, size.height)),
    ));
}

/// Spawn a single controllable lightcycle at the arena centre, heading +X.
/// MVP-only: the full lifecycle (`create_player` / `spawn_player` with
/// deterministic RNG) lives behind this for now.
pub fn setup_local_player(mut commands: Commands) {
    let player = commands
        .spawn((
            Player,
            PlayerId("local".to_string()),
            Position(Vec2::ZERO),
            Direction(Vec2::new(1.0, 0.0)),
            Velocity(Vec2::ZERO),
            SpeedMult::base(),
            PlayerColor(0x00FFCC),
            IsAlive(true),
            TrailPointCount(1),
            TrailPointNextOrder(1),
        ))
        .id();

    commands.spawn((
        TrailPoint,
        TrailPointOrder(0),
        Position(Vec2::ZERO),
        Direction(Vec2::new(1.0, 0.0)),
        ChildOf(player),
    ));
}
