//! Short-horizon bot memory: a fixed ring of freedom/exits samples plus an
//! opponent-history stub.

use serde::{Deserialize, Serialize};

use super::strategy::Action;

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
            opponent: OpponentHistory,
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

/// Stub: real opponent-action modelling is not ported yet.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct OpponentHistory;

impl OpponentHistory {
    pub fn record(&mut self, _action: Action) {}
}
