//! Tron Zero — shared simulation core.
//!
//! Pure bevy_ecs components + systems. No networking, no rendering.
//! Both client and server depend on this crate so the simulation stays
//! identical across peers.

use bevy_ecs::prelude::*;
use glam::Vec2;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Fixed simulation rate. Matches the JS `GameClock` default.
pub const TICK_HZ: f64 = 120.0;
/// Seconds per tick (`1/120`).
pub const TICK_SECS: f32 = 1.0 / 120.0;
/// Milliseconds per tick.
pub const TICK_MS: f32 = 1000.0 / 120.0;

/// Base lightcycle speed in world-units per second.
pub const BASE_SPEED: f32 = 360.0;

/// Maximum rubber meter. Reaches 0 → death.
pub const BASE_RUBBER: f32 = 120.0;
/// Generic per-tick delta scalar (matches JS `DELTA_STUFF`).
pub const DELTA_STUFF: f32 = 12.0;
/// Distance from a wall/trail at which the rubber zone kicks in.
pub const SLOW_DOWN_DISTANCE: f32 = 12.0;
/// Maximum trail arc length in world-units (`200 * tick_ms`).
pub const TRAIL_MAX_LENGTH: f32 = 200.0 * TICK_MS;
/// Turn angle in radians (90°).
pub const ROTATION_ANGLE: f32 = core::f32::consts::FRAC_PI_2;

/// Arena defaults (world units).
pub const ARENA_WIDTH: f32 = 2400.0;
pub const ARENA_HEIGHT: f32 = 2400.0;
// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Marker for a player entity (the lightcycle).
#[derive(Component, Default)]
#[require(Position, Direction, Velocity, SpeedMult, TargetSpeedMult, IsAlive, PlayerColor, Rubber, IsSliding, IsColliding, ShouldHandleDeath, TrailPointCount, TrailPointNextOrder)]
pub struct Player;

/// Stable identity across respawns. Not used in the MVP yet but reserved.
#[derive(Component, Clone, Debug)]
pub struct PlayerId(pub String);

/// World position.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Position(pub Vec2);

/// Unit heading. Constrained to ±X / ±Y; turns are 90° component swaps.
#[derive(Component, Clone, Copy, Debug)]
pub struct Direction(pub Vec2);

impl Default for Direction {
    fn default() -> Self {
        Self(Vec2::new(1.0, 0.0))
    }
}

/// Per-tick displacement × 1000 (Position advances by `Velocity / 1000` each
/// tick). Kept for parity with the JS codebase's fixed-point convention.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Velocity(pub Vec2);

/// Current speed multiplier (1.0 = base speed).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SpeedMult(pub f32);

impl SpeedMult {
    pub const fn base() -> Self {
        Self(1.0)
    }
}

/// Target speed multiplier the actual `SpeedMult` drifts toward (inertia).
/// Boosted while sliding; decays toward 1.0 in open space.
#[derive(Component, Clone, Copy, Debug)]
pub struct TargetSpeedMult(pub f32);

impl Default for TargetSpeedMult {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Packed RGB `(r<<16)|(g<<8)|b`.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PlayerColor(pub u32);

/// Alive flag. Toggled on death, the entity persists for respawn.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct IsAlive(pub bool);

/// Rubber meter. Clamped to `[0, BASE_RUBBER]`. Reaching 0 → death.
#[derive(Component, Clone, Copy, Debug)]
pub struct Rubber(pub f32);

impl Default for Rubber {
    fn default() -> Self {
        Self(BASE_RUBBER)
    }
}

/// True while a side sensor ray is within `SLOW_DOWN_DISTANCE` of an obstacle
/// (sliding/grinding). Triggers acceleration.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct IsSliding(pub bool);

/// True while the front sensor ray is within `SLOW_DOWN_DISTANCE` of an
/// obstacle (rubber zone engaged).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct IsColliding(pub bool);

/// Guards one-shot death handling next Phase 1.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ShouldHandleDeath(pub bool);

/// Mirrors the player's child `TrailPoint` count for cheap zero-checks.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TrailPointCount(pub u32);

// ---------------------------------------------------------------------------
// Trail
// ---------------------------------------------------------------------------

/// Marker for a trail point — a static vertex left at each turn site.
/// Child of the `Player` entity that spawned it.
#[derive(Component, Default)]
#[require(Position, Direction, TrailPointOrder)]
pub struct TrailPoint;

/// Sort key for trail rendering. Assigned monotonically by the parent
/// player's `TrailPointNextOrder` counter.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TrailPointOrder(pub u32);

/// Monotonic counter for the next `TrailPointOrder` value. Lives on the
/// player so it rolls back with the player under lightyear prediction.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TrailPointNextOrder(pub u32);

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Client → server turn input (lightyear `Input` in the full design).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerInput {
    #[default]
    None,
    TurnLeft,
    TurnRight,
}

/// Single-slot pending input consumed each tick. The client's input manager
/// writes here; the turn system drains it.
#[derive(Resource, Default)]
pub struct PendingInput(pub PlayerInput);

// ---------------------------------------------------------------------------
// Arena
// ---------------------------------------------------------------------------

/// Marker for the single arena entity.
#[derive(Component, Default)]
#[require(ArenaSize, WallSegments)]
pub struct Arena;

/// Arena dimensions in world units.
#[derive(Component, Clone, Copy, Debug)]
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
#[derive(Component, Clone, Debug, Default)]
pub struct WallSegments(pub Vec<[f32; 4]>);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Rotate a ±X/±Y heading 90° counter-clockwise: `(x, y) -> (-y, x)`.
#[inline]
pub fn rotate_left(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// Rotate a ±X/±Y heading 90° clockwise: `(x, y) -> (y, -x)`.
#[inline]
pub fn rotate_right(v: Vec2) -> Vec2 {
    Vec2::new(v.y, -v.x)
}

/// Build the four boundary walls of an arena centred at the origin.
pub fn arena_walls(width: f32, height: f32) -> Vec<[f32; 4]> {
    let hw = width * 0.5;
    let hh = height * 0.5;
    // bottom, right, top, left
    vec![
        [-hw, -hh, hw, -hh],
        [hw, -hh, hw, hh],
        [hw, hh, -hw, hh],
        [-hw, hh, -hw, -hh],
    ]
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

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
            TrailPointCount(1), // one initial trail point spawned below
            TrailPointNextOrder(1), // 0 reserved for the initial point below
        ))
        .id();

    // Initial trail point at the spawn location.
    commands.spawn((
        TrailPoint,
        TrailPointOrder(0),
        Position(Vec2::ZERO),
        Direction(Vec2::new(1.0, 0.0)),
        ChildOf(player),
    ));
}

/// Apply a pending turn input: spawn a `TrailPoint` at the current position
/// (capturing the pre-turn heading), then rotate `Direction` by ±90°.
///
/// Runs in `FixedUpdate` before movement so the turn takes effect this tick.
pub fn apply_turn(
    mut pending: ResMut<PendingInput>,
    mut commands: Commands,
    mut players: Query<
        (
            Entity,
            &mut Direction,
            &Position,
            &mut TrailPointNextOrder,
            &mut TrailPointCount,
            &IsAlive,
        ),
        With<Player>,
    >,
) {
    let input = pending.0;
    pending.0 = PlayerInput::None;
    if input == PlayerInput::None {
        return;
    }
    for (entity, mut dir, pos, mut next_order, mut count, alive) in &mut players {
        if !alive.0 {
            continue;
        }
        // Spawn a trail point at the current position with the pre-turn heading.
        commands.spawn((
            TrailPoint,
            TrailPointOrder(next_order.0),
            Position(pos.0),
            Direction(dir.0),
            ChildOf(entity),
        ));
        next_order.0 += 1;
        count.0 += 1;

        dir.0 = match input {
            PlayerInput::TurnLeft => rotate_left(dir.0),
            PlayerInput::TurnRight => rotate_right(dir.0),
            PlayerInput::None => dir.0,
        };
        // Re-normalise to kill any float drift: heading stays axis-aligned.
        dir.0 = dir.0.normalize_or_zero();
    }
}

/// Advance every alive lightcycle by one tick at constant speed.
///
/// `Velocity = Direction * BASE_SPEED * SpeedMult * tick_ms` (displacement
/// × 1000), `Position += Velocity / 1000`. At `SpeedMult = 1` this is
/// `BASE_SPEED` world-units per second.
pub fn move_players(
    mut players: Query<(&mut Position, &mut Velocity, &Direction, &SpeedMult, &IsAlive), With<Player>>,
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

/// Bevy system set ordering for the fixed simulation.
///
/// `Turn` runs before `Move` so a turn input is reflected in the same tick's
/// displacement (matches the JS Phase 1 → Phase 3 ordering).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    Turn,
    Move,
}
