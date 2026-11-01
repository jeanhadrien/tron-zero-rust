//! Fixed simulation rate, lightcycle physics, arena defaults.
//!
//! Mirrors the JS `GameClock` and gameplay tuning constants.

pub const TICK_HZ: f64 = 120.0;
pub const TICK_SECS: f32 = 1.0 / 120.0;
pub const TICK_MS: f32 = 1000.0 / 120.0;

pub const BASE_SPEED: f32 = 360.0;
pub const BASE_RUBBER: f32 = 120.0;
pub const DELTA_STUFF: f32 = 12.0;
pub const SLOW_DOWN_DISTANCE: f32 = 12.0;
/// Wall left behind a rider: ~6.7 s of base-speed riding, 1.2x the arena side.
/// Long enough that 2-8 riders keep cutting each other off.
pub const TRAIL_MAX_LENGTH: f32 = 2400.0;
pub const ROTATION_ANGLE: f32 = core::f32::consts::FRAC_PI_2;

/// Square arena (bots assume it): 5.6 s edge to edge at base speed, 2.8 s at the cap.
pub const ARENA_WIDTH: f32 = 2000.0;
pub const ARENA_HEIGHT: f32 = 2000.0;

// --- Speed: grinding is the only way to go faster -------------------------

/// Hard ceiling on cruise speed (x BASE_SPEED). At the cap a stuck rider dies
/// in ~12 ticks, about the turn late window, so the cap is the fastest the
/// rubber can still be a fair net.
pub const MAX_SPEED_MULT: f32 = 2.0;
/// Side-obstacle distance where grinding starts to pay (wider than the front
/// slowdown zone). Closer is faster: the goal speed scales linearly to the cap.
pub const SLIDE_DISTANCE: f32 = 24.0;
/// Boundary walls count as this fraction of their real closeness, so
/// riding the arena edge is a weaker accelerator than riding trails.
pub const ARENA_WALL_GRIND: f32 = 0.4;
/// Cruise speed gained per second while below the grind goal: ~2 s of tight
/// grinding for base to cap.
pub const SLIDE_ACCEL_PER_SEC: f32 = 0.5;
/// Cruise speed lost per second in open space or above the goal: boost
/// fades over ~2 s, so it must be earned continuously.
pub const COAST_DECEL_PER_SEC: f32 = 0.5;
/// Cruise speed lost per second while pressed against an obstacle: crashing
/// bleeds the boost.
pub const CRASH_BRAKE_PER_SEC: f32 = 3.0;
pub const SLIDE_ACCEL: f32 = SLIDE_ACCEL_PER_SEC * TICK_SECS;
pub const COAST_DECEL: f32 = COAST_DECEL_PER_SEC * TICK_SECS;
pub const CRASH_BRAKE: f32 = CRASH_BRAKE_PER_SEC * TICK_SECS;

// --- Rubber: a scarce panic buffer ----------------------------------------

/// Drain per tick is `RUBBER_DRAIN * (1 + cruise)^3`: ~0.35 s to die at base
/// speed, ~0.1 s at the cap.
pub const RUBBER_DRAIN: f32 = DELTA_STUFF * 0.03;
/// Regen per tick outside the slowdown zone: 18/s, full in ~6.7 s, so a
/// wall tap is cheap but camping on the rubber is not.
pub const RUBBER_REGEN: f32 = 0.15;
