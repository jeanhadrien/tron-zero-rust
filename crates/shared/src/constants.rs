//! Fixed simulation rate, lightcycle physics, arena defaults.
//!
//! Mirrors the JS `GameClock` and gameplay tuning constants.

pub const TICK_HZ: f64 = 120.0;
pub const TICK_SECS: f32 = 1.0 / 120.0;
pub const TICK_MS: f32 = 1000.0 / 120.0;

pub const BASE_SPEED: f32 = 360.0;
pub const BASE_RUBBER: f32 = 120.0;
/// Minimum width of the rubber zone in front of a rider; the zone widens with
/// speed (see `RUBBER_APPROACH`), this is its floor.
pub const SLOW_DOWN_DISTANCE: f32 = 12.0;
/// Wall left behind a rider: ~6.7 s of base-speed riding, 1.2x the arena side.
/// Long enough that 2-8 riders keep cutting each other off.
pub const TRAIL_MAX_LENGTH: f32 = 2400.0;
pub const ROTATION_ANGLE: f32 = core::f32::consts::FRAC_PI_2;

/// Square arena (bots assume it): 5.6 s edge to edge at base speed, 1.9 s at the cap.
pub const ARENA_WIDTH: f32 = 2000.0;
pub const ARENA_HEIGHT: f32 = 2000.0;

// --- Speed: grinding is the only way to go faster -------------------------
//
// Armagetron-style inertia: cruise speed `c` (x BASE_SPEED) follows
// `dc = GRIND_ACCEL * grind + (1 - c) * rate` per tick, where `grind` is the
// sum of both side strengths (0..=2) and `rate` pulls `c` back to 1.0
// (`SPEED_DECAY` above, `SPEED_RECOVER` below). The terminal speed is
// `1 + GRIND_ACCEL / SPEED_DECAY * grind`, clamped to `MAX_SPEED_MULT`.

/// Hard ceiling on cruise speed (x BASE_SPEED). Rubber is scaled so a rider
/// stuck at the cap still outlives the turn late window.
pub const MAX_SPEED_MULT: f32 = 3.0;
/// Side-obstacle distance where grinding stops paying (0.2 s of base travel).
pub const GRIND_RANGE: f32 = 72.0;
/// Offset of the hyperbolic grind falloff `1/(d + GRIND_OFFSET)`: smaller is
/// more convex (touching pays much more than hovering).
pub const GRIND_OFFSET: f32 = 24.0;
/// Scales `1/(d + OFFSET) - 1/(RANGE + OFFSET)` to 1 touching (0 at range).
pub const GRIND_NORM: f32 = 1.0 / (1.0 / GRIND_OFFSET - 1.0 / (GRIND_RANGE + GRIND_OFFSET));
/// Boundary walls pay this fraction of the grind strength a trail would at
/// the same distance, so one side at contact tops out at
/// `1 + GRIND_ACCEL_PER_SEC / SPEED_DECAY_PER_SEC * ARENA_WALL_GRIND`.
pub const ARENA_WALL_GRIND: f32 = 0.4;
/// Own trail only grinds from this many segments behind the head. Nearer
/// legs are a U-turn's outbound leg a step away, which would pay ~0.85.
pub const OWN_GRIND_MIN_FROM_HEAD: usize = 3;
/// Cruise speed gained per second per unit of grind (touching one trail = 1).
pub const GRIND_ACCEL_PER_SEC: f32 = 0.8;
/// Rate (per second) the cruise speed's excess over 1.0 fades.
pub const SPEED_DECAY_PER_SEC: f32 = 0.8;
/// Rate (per second) a cruise speed below 1.0 (turn penalty) recovers.
pub const SPEED_RECOVER_PER_SEC: f32 = 5.0;
pub const GRIND_ACCEL: f32 = GRIND_ACCEL_PER_SEC * TICK_SECS;
pub const SPEED_DECAY: f32 = SPEED_DECAY_PER_SEC * TICK_SECS;
pub const SPEED_RECOVER: f32 = SPEED_RECOVER_PER_SEC * TICK_SECS;
/// Cruise speed is multiplied by this on every executed turn.
pub const TURN_SPEED_FACTOR: f32 = 0.95;
/// Floor of the turn penalty, so spamming turns cannot stall a rider.
pub const MIN_SPEED_MULT: f32 = 0.8;

// --- Rubber: a scarce panic buffer ----------------------------------------

/// In the rubber zone the rider closes this fraction of the remaining gap
/// per tick (Armagetron's `1 - e^(-40 dt)`, as a literal: no `exp` in the sim).
/// The zone is as wide as the step that could not be taken without it
/// (`step / RUBBER_APPROACH`), at least `SLOW_DOWN_DISTANCE`.
pub const RUBBER_APPROACH: f32 = 0.28;
/// Rubber lost per unit of distance not travelled, at base speed; scaled by
/// `2 / (1 + cruise)` so faster riders pay less per unit but still die sooner:
/// ~22 ticks at base, ~14 at the cap, both after contact.
pub const RUBBER_COST: f32 = 1.8;
/// Rubber regained per second is `(rubber + RUBBER_REGEN_FLOOR) * this`,
/// always (also while grinding): proportional to what is left, so a burned
/// rider recovers slowly and a barely scratched one is topped up fast.
pub const RUBBER_REGEN_PER_SEC: f32 = 0.3;
/// Keeps a near-empty rider recovering at all (regen is proportional to
/// `rubber + floor`): 8 rubber is about 1.5 ticks of drain at the wall.
pub const RUBBER_REGEN_FLOOR: f32 = 8.0;
pub const RUBBER_REGEN: f32 = RUBBER_REGEN_PER_SEC * TICK_SECS;
