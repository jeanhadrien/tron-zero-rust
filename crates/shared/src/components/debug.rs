//! Replicated bot introspection snapshot for the debug overlay.
//!
//! Written by the bot driver of a server (one update per bot planning
//! round) and drawn by the client's `draw_bot_debug`. Gated end to end by
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

/// Decision mode ids (mirror `bots::planner::Mode`).
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
    /// Action decided for this tick (`ACTION_*`).
    pub action: u8,
    /// Best objective value among the candidate maneuvers that start with
    /// Hold, Left, Right (a delayed turn counts as Hold).
    pub scores: [f32; 3],
    /// Proximity threat in `[0, 1]`.
    pub threat: f32,
    /// Room (grid cells) left from where the chosen maneuver ends.
    pub area: u32,
    /// Candidates whose line runs the whole horizon unobstructed.
    pub open: u8,
    /// Misjudgment of distances and room, `0` = none.
    pub noise: f32,
    /// The chosen maneuver's space, reach and clearance terms, each `0..=1`.
    pub space: f32,
    pub reach: f32,
    pub clear: f32,
    /// Planning horizon and reaction cadence, in ticks.
    pub horizon: u32,
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
            area: 1,
            open: 0,
            noise: 0.0,
            space: 0.0,
            reach: 0.0,
            clear: 0.0,
            horizon: 1,
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

    /// Index of the best first move (argmax over `scores`).
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
