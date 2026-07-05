//! Player simulation systems: turn application, movement, collision.

use bevy_ecs::prelude::*;
use glam::Vec2;

use crate::ActionState;
use crate::components::arena::{Arena, ArenaSize};
use crate::components::player::{
    Direction, IsAlive, Player, PlayerInput, Position, ShouldHandleDeath, SpeedMult,
    TrailPointCount, TrailPointNextOrder, Velocity,
};
use crate::components::trail::{TrailPoint, TrailPointOrder};
use crate::constants::{BASE_SPEED, TICK_MS};
use crate::math::{rotate_left, rotate_right};

/// Apply a pending turn input: spawn a `TrailPoint` at the current position
/// (capturing the pre-turn heading), then rotate `Direction` by ±90°.
///
/// Each player entity carries its own `ActionState<PlayerInput>` — the client
/// writes the local player's input, the server receives and injects it via
/// lightyear's input system. Remote/non-owner entities stay at `None`.
///
/// Runs in `FixedUpdate` before movement so the turn takes effect this tick.
#[allow(clippy::type_complexity)]
pub fn apply_turn(
    mut commands: Commands,
    mut players: Query<
        (
            Entity,
            &mut Direction,
            &Position,
            &mut TrailPointNextOrder,
            &mut TrailPointCount,
            &mut ActionState<PlayerInput>,
            &IsAlive,
        ),
        With<Player>,
    >,
) {
    for (entity, mut dir, pos, mut next_order, mut count, mut input, alive) in &mut players {
        if !alive.0 {
            continue;
        }
        let turn = input.0;
        if turn == PlayerInput::None {
            continue;
        }
        commands.spawn((
            TrailPoint,
            TrailPointOrder(next_order.0),
            Position(pos.0),
            Direction(dir.0),
            ChildOf(entity),
        ));
        next_order.0 += 1;
        count.0 += 1;

        dir.0 = match turn {
            PlayerInput::TurnLeft => rotate_left(dir.0),
            PlayerInput::TurnRight => rotate_right(dir.0),
            PlayerInput::None => dir.0,
        };
        dir.0 = dir.0.normalize_or_zero();
        input.0 = PlayerInput::None;
    }
}

/// Advance every alive lightcycle by one tick at constant speed.
///
/// `Velocity = Direction * BASE_SPEED * SpeedMult * tick_ms` (displacement
/// × 1000), `Position += Velocity / 1000`. At `SpeedMult = 1` this is
/// `BASE_SPEED` world-units per second.
pub fn move_players(
    mut players: Query<
        (
            &mut Position,
            &mut Velocity,
            &Direction,
            &SpeedMult,
            &IsAlive,
        ),
        With<Player>,
    >,
) {
    for (mut pos, mut vel, dir, speed, alive) in &mut players {
        if !alive.0 {
            vel.0 = Vec2::ZERO;
            continue;
        }
        let displacement_x1000 = dir.0 * (BASE_SPEED * speed.0 * TICK_MS);
        vel.0 = displacement_x1000;
        pos.0 += displacement_x1000 / 1000.0;
    }
}

/// Clamp players to the arena boundaries. If a player is outside, mark them
/// for death and clamp position to the edge.
pub fn collide_with_arena(
    arena: Query<&ArenaSize, With<Arena>>,
    mut players: Query<
        (
            &mut Position,
            &mut Velocity,
            &mut ShouldHandleDeath,
            &mut IsAlive,
        ),
        With<Player>,
    >,
) {
    let Ok(arena) = arena.single() else {
        return;
    };
    let hw = arena.width * 0.5;
    let hh = arena.height * 0.5;

    for (mut pos, mut vel, mut should_die, mut alive) in &mut players {
        if !alive.0 {
            continue;
        }
        let clamped = pos.0.clamp(Vec2::new(-hw, -hh), Vec2::new(hw, hh));
        if clamped != pos.0 {
            pos.0 = clamped;
            vel.0 = Vec2::ZERO;
            alive.0 = false;
            should_die.0 = true;
        }
    }
}
