//! Per-rider measurements a lab run collects on the side. They never reach the
//! trace, so goldens do not depend on them; tournaments read them.

use shared::RoundEnd;

/// A living enemy this close (world units) counts as company: the rider is
/// not idle. About a quarter of the arena width.
pub const IDLE_RANGE: f32 = 500.0;

/// What one rider did over a run. All tick counts are ticks the rider ended
/// alive.
#[derive(Clone, Debug, Default)]
pub struct RiderStats {
    pub alive_ticks: u64,
    /// Ticks spent in the rubber zone (facing an obstacle).
    pub rubber_ticks: u64,
    /// Ticks with no living enemy within `IDLE_RANGE` and the nearest enemy
    /// not getting closer.
    pub idle_ticks: u64,
    /// Sum of the speed multiplier over `alive_ticks`.
    pub speed_sum: f64,
    /// Length in ticks of every life: a death ends one, so does the round's
    /// end for a survivor.
    pub lifetimes: Vec<u32>,
}

/// How one round of a match ended. Riders are lab rider indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundResult {
    pub round: u32,
    /// First and last tick of the round, both inclusive.
    pub start: u32,
    pub end: u32,
    pub reason: RoundEnd,
    pub winner: Option<usize>,
}
