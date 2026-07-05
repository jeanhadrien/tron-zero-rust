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
pub const TRAIL_MAX_LENGTH: f32 = 200.0 * TICK_MS;
pub const ROTATION_ANGLE: f32 = core::f32::consts::FRAC_PI_2;

pub const ARENA_WIDTH: f32 = 2400.0;
pub const ARENA_HEIGHT: f32 = 2400.0;
