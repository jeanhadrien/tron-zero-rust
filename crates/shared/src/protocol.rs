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

/// Bumped whenever the wire protocol or gameplay contract changes in a way an
/// old peer cannot follow. Exchanged in [`SessionHello`]; a mismatch is
/// rejected with [`JoinRejection::VersionMismatch`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest accepted player name, in characters.
pub const MAX_NAME_CHARS: usize = 16;
/// Name used when the requested one is empty or sanitizes to nothing.
pub const DEFAULT_NAME: &str = "Player";

/// First message of every session (client -> server): who is joining, on which
/// protocol. The server only spawns a rider after accepting it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHello {
    pub protocol: u32,
    pub name: String,
}

/// Why the server refused a [`SessionHello`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinRejection {
    VersionMismatch { server: u32 },
    ServerFull { max_players: u32 },
}

impl std::fmt::Display for JoinRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionMismatch { server } => write!(
                f,
                "Incompatible game version (server protocol {server}, yours {PROTOCOL_VERSION}). \
                 Update the client or pick another server."
            ),
            Self::ServerFull { max_players } => {
                write!(f, "Server is full ({max_players} players).")
            }
        }
    }
}

/// Server -> client answer to [`SessionHello`]. `Accepted` carries the name
/// the server settled on (sanitized, de-duplicated).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelloReply {
    Accepted { name: String },
    Rejected(JoinRejection),
}

/// Normalize a player-supplied name: drop control characters, trim, cap the
/// length, and fall back to [`DEFAULT_NAME`] when nothing is left.
pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let capped: String = cleaned.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = capped.trim_end();
    if name.is_empty() {
        DEFAULT_NAME.to_owned()
    } else {
        name.to_owned()
    }
}

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

/// One-shot turn delivery: one physical key press = one sequenced command.
///
/// The client stamps `tick` with the tick its *prediction* applies the turn at
/// (`LocalTimeline` + input delay, read in `FixedPreUpdate`). Lightyear drives
/// that timeline from `InputTimeline`, which runs ~RTT/2 + jitter margin AHEAD
/// of the server, so a healthy command arrives before `tick` and the server
/// executes it on exactly that tick. A command that arrives late runs on the
/// first free tick after receipt if it is within [`TURN_LATE_WINDOW_TICKS`] of
/// its intended tick, otherwise it is rejected. Missing input executes
/// nothing; the server never rewrites past ticks.
///
/// Carries NO player/entity id; the server derives ownership from the link,
/// like respawns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCommand {
    /// Monotonic per client; never reset within a connection.
    pub seq: u64,
    pub direction: TurnDirection,
    /// `LifeGeneration` of the rider at press time; mismatches are stale.
    pub generation: u128,
    /// Intended execution tick, as predicted by the client.
    pub tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnRejectReason {
    /// `seq` not above the highest already accepted for this life.
    Duplicate,
    StaleLife,
    NotAlive,
    QueueFull,
    /// Intended tick is more than [`TURN_MAX_LEAD_TICKS`] ahead of the server.
    TooFarAhead,
    /// Arrived (or waited) more than [`TURN_LATE_WINDOW_TICKS`] after its
    /// intended tick.
    TooLate,
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
/// How far past its intended tick a turn may still execute (100 ms at 120 Hz).
/// Late execution shifts the corner by `lateness * 3` units and costs the
/// client a correction; beyond the window the turn is rejected instead.
pub const TURN_LATE_WINDOW_TICKS: u32 = 12;
/// How far ahead of the server's tick a command may be scheduled. Mirrors
/// Lightyear's own `MAX_INPUT_LOOKAHEAD_TICKS` for native inputs.
pub const TURN_MAX_LEAD_TICKS: i32 = 64;

pub fn register_protocol(app: &mut App) {
    app.add_channel::<SessionChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    // The hello pair is registered first so its wire ids stay stable across
    // builds that add or reorder later messages: a mismatched peer can still
    // decode the handshake and be told why it is refused.
    app.register_message::<SessionHello>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<HelloReply>()
        .add_direction(NetworkDirection::ServerToClient);
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
    // Final at spawn: the server admits a player (fixing the name) before
    // spawning the rider, so late joiners get it with the initial state.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_controls_trims_and_caps() {
        assert_eq!(sanitize_name("  Neo \u{7}\n"), "Neo");
        assert_eq!(sanitize_name("a\tb"), "ab");
        assert_eq!(sanitize_name(""), DEFAULT_NAME);
        assert_eq!(sanitize_name(" \u{0}  "), DEFAULT_NAME);
        assert_eq!(
            sanitize_name(&"x".repeat(40)).chars().count(),
            MAX_NAME_CHARS
        );
        // Multi-byte characters are counted, never sliced mid-codepoint.
        assert_eq!(
            sanitize_name(&"é".repeat(40)).chars().count(),
            MAX_NAME_CHARS
        );
    }
}
