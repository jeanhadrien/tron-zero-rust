//! Replicated bot introspection snapshot for the debug overlay.
//!
//! Written by the server's `bot_brain_input` (one update per bot decision
//! epoch) and drawn by the client's `draw_bot_debug`. Gated end to end by
//! `TRON_BOT_DEBUG=1`: when the env var is off no entity ever carries this
//! component, so replication bandwidth and client cost are both zero.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Bit in [`BotDebug::flags`] when the bot and its foe face each other.
pub const FLAG_HEAD_ON: u8 = 0b01;
/// Bit in [`BotDebug::flags`] when either rider faces the other in range.
pub const FLAG_CONVERGING: u8 = 0b10;

/// Decision mode ids (mirror `server::bot::strategy::Mode`).
pub const MODE_ESCAPE: u8 = 0;
pub const MODE_SURVIVAL: u8 = 1;
pub const MODE_ATTACK: u8 = 2;
pub const MODE_EXPLORE: u8 = 3;

/// Action ids (mirror `Action::index`: Hold = 0, Left = 1, Right = 2).
pub const ACTION_HOLD: u8 = 0;
pub const ACTION_LEFT: u8 = 1;
pub const ACTION_RIGHT: u8 = 2;

/// Per-bot decision snapshot. Plain data: no logic, validated by the writer.
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Reflect)]
pub struct BotDebug {
    /// Decision mode (`MODE_*`).
    pub mode: u8,
    /// Decided action (`ACTION_*`).
    pub action: u8,
    /// Raw strategy scores for `[Hold, Left, Right]`.
    pub scores: [f32; 3],
    /// Proximity threat in `[0, 1]`.
    pub threat: f32,
    /// Reachable cells within the BFS budget.
    pub freedom: u32,
    /// Open exits (0-4) from the rider's cell.
    pub exits: u8,
    /// Enemy turn-rate estimate in `[0, 1]`.
    pub turn_prob: f32,
    /// Policy knobs active for this decision.
    pub aggression: f32,
    pub caution: f32,
    pub greed: f32,
    pub lookahead: u32,
    pub reaction: u32,
    /// Nearest foe position (zero when no foe is alive).
    pub target: Vec2,
    /// Nearest foe heading (zero when no foe is alive).
    pub foe_dir: Vec2,
    /// The 8-ray sensor fan (infinite = no hit).
    pub rays: [f32; 8],
    /// `FLAG_*` bits for head-on / converging geometry.
    pub flags: u8,
}

impl Default for BotDebug {
    fn default() -> Self {
        Self {
            mode: MODE_EXPLORE,
            action: ACTION_HOLD,
            scores: [0.0; 3],
            threat: 0.0,
            freedom: 1,
            exits: 0,
            turn_prob: 0.0,
            aggression: 0.0,
            caution: 0.0,
            greed: 0.0,
            lookahead: 1,
            reaction: 1,
            target: Vec2::ZERO,
            foe_dir: Vec2::ZERO,
            rays: [f32::INFINITY; 8],
            flags: 0,
        }
    }
}

impl BotDebug {
    /// True when the bot and its foe face each other nose to nose.
    pub fn head_on(self) -> bool {
        self.flags & FLAG_HEAD_ON != 0
    }

    /// True when either rider faces the other inside duel range.
    pub fn converging(self) -> bool {
        self.flags & FLAG_CONVERGING != 0
    }

    /// Index of the winning fork (argmax over `scores`).
    pub fn winner(self) -> usize {
        let mut best = 0;
        for i in 1..3 {
            if self.scores[i] > self.scores[best] {
                best = i;
            }
        }
        best
    }
}
