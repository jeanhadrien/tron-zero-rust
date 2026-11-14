//! Replicated state of the rounds game mode (server-authoritative display data).
//!
//! Never read by `simulate_players`: the match is decided by the server's round
//! controller and only shown by clients.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// Where the match is, as clients see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum MatchPhase {
    /// No round runs: free riding with normal respawns (the mode when the
    /// server has no rounds, or while it waits for a second rider).
    #[default]
    Free,
    /// Countdown before round 1 of a match; nobody is riding.
    Starting,
    /// A round is being played.
    Playing,
    /// A round just ended; `winner` of [`MatchStatus`] says who took it.
    Intermission,
    /// The last round ended; `standings` are final.
    MatchOver,
}

/// The single match entity of a server in rounds mode (absent in free-for-all).
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
pub struct MatchStatus {
    /// Rounds per match.
    pub rounds: u32,
    /// Current (or, during `Starting`, upcoming) round, from 1; 0 when `Free`.
    pub round: u32,
    pub phase: MatchPhase,
    /// Whole seconds left on the clock of this phase (the round time limit
    /// while `Playing`, the break otherwise); `None` without a limit.
    pub secs_left: Option<u16>,
    /// Winner of the round that just ended; `None` for a draw. Only meaningful
    /// in `Intermission` and `MatchOver`.
    pub winner: Option<String>,
    /// Round wins per rider (name, wins), most wins first.
    pub standings: Vec<(String, u32)>,
}
