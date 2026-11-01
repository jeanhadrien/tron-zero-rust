//! Short-horizon bot memory: a fixed ring of freedom/exits samples plus a
//! deterministic opponent turn-rate tracker.

use std::collections::HashMap;

use bevy::prelude::Vec2;
use serde::{Deserialize, Serialize};

pub const MEMORY_SAMPLES: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotMemory {
    freedom: [u32; MEMORY_SAMPLES],
    exits: [u8; MEMORY_SAMPLES],
    len: usize,
    head: usize,
    pub opponent: OpponentHistory,
}

impl Default for BotMemory {
    fn default() -> Self {
        Self {
            freedom: [0; MEMORY_SAMPLES],
            exits: [0; MEMORY_SAMPLES],
            len: 0,
            head: 0,
            opponent: OpponentHistory::default(),
        }
    }
}

impl BotMemory {
    pub fn observe(&mut self, freedom: u32, exits: u8) {
        self.freedom[self.head] = freedom;
        self.exits[self.head] = exits;
        self.head = (self.head + 1) % MEMORY_SAMPLES;
        self.len = (self.len + 1).min(MEMORY_SAMPLES);
    }

    /// Newest-minus-oldest freedom per step, normalised to `[-1, 1]`.
    pub fn trend(&self) -> f32 {
        if self.len < 2 {
            return 0.0;
        }
        let newest = (self.head + MEMORY_SAMPLES - 1) % MEMORY_SAMPLES;
        let oldest = (self.head + MEMORY_SAMPLES - self.len) % MEMORY_SAMPLES;
        let span = (self.len - 1) as f32;
        ((self.freedom[newest] as f32 - self.freedom[oldest] as f32) / span / 32.0).clamp(-1.0, 1.0)
    }
}

/// Turn-event ring length for [`OpponentHistory`].
pub const TURN_RING: usize = 16;

/// Deterministic turn-rate tracker over observed enemy headings.
///
/// Keyed by rider id (entity bits, stable across forecasts): remembers each
/// rider's last cardinal direction and pushes a turn/straight event into a
/// fixed ring whenever a new heading is observed. No randomness, no
/// wall-clock; `turn_prob` is the raw ring mean (`0.0` when empty, i.e.
/// assume-straight).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OpponentHistory {
    last_dirs: HashMap<u64, u8>,
    ring: [u8; TURN_RING],
    head: usize,
    len: usize,
}

impl OpponentHistory {
    /// Record one observed heading. First sighting only stores the direction;
    /// later sightings push `1` on a heading change, `0` otherwise.
    pub fn observe(&mut self, rider_id: u64, dir: Vec2) {
        let code = encode_dir(dir);
        match self.last_dirs.get(&rider_id) {
            None => {
                self.last_dirs.insert(rider_id, code);
            }
            Some(&prev) => {
                self.push(if prev == code { 0 } else { 1 });
                self.last_dirs.insert(rider_id, code);
            }
        }
    }

    /// Observed turn rate in `[0, 1]`; `0.0` with no samples (assume-straight).
    pub fn turn_prob(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let turns: u32 = self.ring[..self.len].iter().map(|&e| e as u32).sum();
        turns as f32 / self.len as f32
    }

    fn push(&mut self, event: u8) {
        self.ring[self.head] = event;
        self.head = (self.head + 1) % TURN_RING;
        self.len = (self.len + 1).min(TURN_RING);
    }
}

/// Encode a cardinal heading as a small tag so memory stays serde-friendly
/// without depending on `Vec2` serialization.
fn encode_dir(dir: Vec2) -> u8 {
    if dir.x > 0.5 {
        0
    } else if dir.x < -0.5 {
        1
    } else if dir.y > 0.5 {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_history_assumes_straight() {
        assert_eq!(OpponentHistory::default().turn_prob(), 0.0);
    }

    #[test]
    fn straight_running_stays_zero_and_turns_raise_prob() {
        let mut history = OpponentHistory::default();
        history.observe(7, Vec2::X);
        history.observe(7, Vec2::X);
        history.observe(7, Vec2::X);
        assert_eq!(history.turn_prob(), 0.0);
        history.observe(7, Vec2::Y);
        assert!((history.turn_prob() - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn first_sighting_is_not_a_sample() {
        let mut history = OpponentHistory::default();
        history.observe(1, Vec2::X);
        assert_eq!(history.turn_prob(), 0.0);
        history.observe(2, Vec2::NEG_X);
        assert_eq!(history.turn_prob(), 0.0);
    }
}
