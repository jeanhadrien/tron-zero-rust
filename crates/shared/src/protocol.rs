//! Lightyear protocol registration — called by both client and server before
//! spawning the link entity.
//!
//! Registers all replicated components, prediction targets, and the input type
//! so the protocol checksums match across peers.

use crate::*;
use bevy_app::App;
use lightyear::prelude::input::native::InputPlugin;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RespawnRequest {
    pub generation: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RespawnOutcome {
    Accepted,
    NotEligible,
    NoSafeSpace,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RespawnReply {
    pub generation: u128,
    pub outcome: RespawnOutcome,
}

pub struct RespawnChannel;

pub const SESSION_HEARTBEAT_SECS: f64 = 1.0;
pub const SESSION_TIMEOUT_SECS: f64 = 5.0;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum SessionRequest {
    Heartbeat { nonce: u64 },
    Leave { nonce: u64 },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SessionReply {
    pub nonce: u64,
}

pub struct SessionChannel;

/// One-shot turn delivery (phantom-turn fix).
///
/// DESIGN DECISION — bounded late execution, not strict intended-tick
/// rejection: our client uses `no_input_delay`, so every turn arrives "late"
/// relative to its prediction tick under any RTT. Rejecting late turns would
/// drop every turn. Instead each physical press is a sequenced command the
/// server executes at most once, in order, on the next available fixed tick(s)
/// after receipt. Missing input executes nothing; duplicates execute nothing.
/// The server never rewrites past ticks, so there is no server rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnDirection {
    Left,
    Right,
}

impl TurnDirection {
    /// Tag the turn with the life it was pressed in so the sim's
    /// `eligible_turn` gate only fires it for that life.
    pub fn to_input(self, generation: u128) -> PlayerInput {
        match self {
            Self::Left => PlayerInput::TurnLeftFor(generation),
            Self::Right => PlayerInput::TurnRightFor(generation),
        }
    }
}

/// Client → server turn command. Carries NO player/entity id; the server
/// derives ownership from the link, like respawns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCommand {
    /// Per-client-link monotonic sequence, reset only on disconnect.
    pub seq: u64,
    pub direction: TurnDirection,
    /// `LifeGeneration` of the rider at press time; mismatches are stale.
    pub generation: u128,
    /// `LocalTimeline` tick at send (diagnostics only, never execution).
    pub client_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnRejectReason {
    Duplicate,
    StaleLife,
    NotAlive,
    QueueFull,
    Expired,
    NotEligible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOutcome {
    Applied { server_tick: u32 },
    Rejected { reason: TurnRejectReason },
}

/// Server → client ack for every accepted-or-rejected command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAck {
    pub seq: u64,
    pub outcome: TurnOutcome,
}

pub struct TurnChannel;

/// Max buffered turns per player; beyond this new commands are rejected with
/// `QueueFull` (accepted commands are never silently dropped).
pub const TURN_QUEUE_CAP: usize = 32;
/// Queued commands older than ~1s (120 ticks at 120 Hz) expire with `Expired`.
pub const TURN_QUEUE_TTL_TICKS: u32 = 120;

pub fn register_protocol(app: &mut App) {
    app.add_channel::<SessionChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    app.register_message::<SessionRequest>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<SessionReply>()
        .add_direction(NetworkDirection::ServerToClient);
    app.add_channel::<RespawnChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    app.register_message::<RespawnRequest>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<RespawnReply>()
        .add_direction(NetworkDirection::ServerToClient);
    // --- One-shot turns (phantom-turn fix) ---
    app.add_channel::<TurnChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    app.register_message::<TurnCommand>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<TurnAck>()
        .add_direction(NetworkDirection::ServerToClient);
    // --- Input ---
    app.add_plugins(InputPlugin::<PlayerInput>::default());

    // --- Predicted components (client predicts, server corrects) ---
    app.component::<Position>()
        .replicate()
        .predict()
        .add_interpolation_with(step);
    app.component::<Direction>()
        .replicate()
        .predict()
        .add_interpolation_with(step);
    app.component::<Velocity>().replicate().predict();
    app.component::<SpeedMult>().replicate().predict();
    app.component::<TargetSpeedMult>().replicate().predict();
    app.component::<Rubber>().replicate().predict();
    app.component::<IsAlive>()
        .replicate()
        .predict()
        .add_interpolation_with(step);
    app.component::<IsSliding>().replicate().predict();
    app.component::<IsColliding>().replicate().predict();
    app.component::<ShouldHandleDeath>().replicate().predict();
    app.component::<LifeGeneration>().replicate().predict();
    app.component::<Trail>()
        .replicate()
        .predict()
        .add_interpolation_with(step);

    // --- Replicated-only (no prediction needed) ---
    app.component::<BotDebug>().replicate();
    app.component::<Player>().replicate_once();
    app.component::<PlayerId>().replicate_once();
    app.component::<DisplayName>().replicate_once();
    app.component::<PlayerColor>().replicate_once();

    app.component::<Arena>().replicate_once();
    app.component::<ArenaSize>().replicate_once();
    app.component::<WallSegments>().replicate_once();

    // ActionState is local per-tick state (written by input systems, read by
    // simulate_players). Not replicated — the lightyear input system handles
    // transmitting the value. Both sides add it manually on their entity.
}

// Discrete snapshots keep cardinal turns and trail heads on the same tick.
fn step<T>(start: T, end: T, t: f32) -> T {
    if t >= 1.0 { end } else { start }
}
