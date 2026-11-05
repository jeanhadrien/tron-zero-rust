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

/// Bumped by hand whenever the wire protocol or the gameplay *logic* changes in
/// a way an old peer cannot follow. Changes to the sim constants listed in
/// [`SIM_CONSTANTS`] need no bump: they change [`PROTOCOL_ID`] by themselves.
///
/// Physics/simulation *logic* changes (grind rules, collision, rubber, turn
/// handling, anything in `simulate_players`) are NOT captured by the constant
/// fingerprint and REQUIRE a manual bump here: otherwise mixed builds connect
/// fine and then silently mispredict.
pub const PROTOCOL_VERSION: u32 = 5;

/// Name and value of every constant the client and server must agree on to
/// simulate identically (`constants.rs` in full, plus the turn timing rules).
/// A test fails when a `pub const` in `constants.rs` is missing from this list,
/// so a retune cannot silently stay wire-compatible.
macro_rules! sim_constants {
    ($($name:ident => $bits:expr),+ $(,)?) => {
        pub const SIM_CONSTANTS: &[(&str, u64)] = &[$((stringify!($name), $bits)),+];
    };
}

sim_constants! {
    TICK_HZ => TICK_HZ.to_bits(),
    TICK_SECS => TICK_SECS.to_bits() as u64,
    TICK_MS => TICK_MS.to_bits() as u64,
    BASE_SPEED => BASE_SPEED.to_bits() as u64,
    BASE_RUBBER => BASE_RUBBER.to_bits() as u64,
    DELTA_STUFF => DELTA_STUFF.to_bits() as u64,
    SLOW_DOWN_DISTANCE => SLOW_DOWN_DISTANCE.to_bits() as u64,
    TRAIL_MAX_LENGTH => TRAIL_MAX_LENGTH.to_bits() as u64,
    ROTATION_ANGLE => ROTATION_ANGLE.to_bits() as u64,
    ARENA_WIDTH => ARENA_WIDTH.to_bits() as u64,
    ARENA_HEIGHT => ARENA_HEIGHT.to_bits() as u64,
    MAX_SPEED_MULT => MAX_SPEED_MULT.to_bits() as u64,
    SLIDE_DISTANCE => SLIDE_DISTANCE.to_bits() as u64,
    ARENA_WALL_GRIND => ARENA_WALL_GRIND.to_bits() as u64,
    SLIDE_ACCEL_PER_SEC => SLIDE_ACCEL_PER_SEC.to_bits() as u64,
    COAST_DECEL_PER_SEC => COAST_DECEL_PER_SEC.to_bits() as u64,
    CRASH_BRAKE_PER_SEC => CRASH_BRAKE_PER_SEC.to_bits() as u64,
    SLIDE_ACCEL => SLIDE_ACCEL.to_bits() as u64,
    COAST_DECEL => COAST_DECEL.to_bits() as u64,
    CRASH_BRAKE => CRASH_BRAKE.to_bits() as u64,
    RUBBER_DRAIN => RUBBER_DRAIN.to_bits() as u64,
    RUBBER_REGEN => RUBBER_REGEN.to_bits() as u64,
    TURN_LATE_WINDOW_TICKS => TURN_LATE_WINDOW_TICKS as u64,
    TURN_MAX_LEAD_TICKS => TURN_MAX_LEAD_TICKS as u64,
    TURN_QUEUE_CAP => TURN_QUEUE_CAP as u64,
    TURN_BATCH_MAX => TURN_BATCH_MAX as u64,
    TURN_RESEND_TICKS => TURN_RESEND_TICKS as u64,
}

/// 64-bit FNV-1a over the version and every [`SIM_CONSTANTS`] value, folded to
/// 32 bits. Computed at compile time.
const fn protocol_id() -> u32 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut word = PROTOCOL_VERSION as u64;
    let mut next = 0;
    loop {
        let mut byte = 0;
        while byte < 8 {
            hash = (hash ^ ((word >> (8 * byte)) & 0xff)).wrapping_mul(PRIME);
            byte += 1;
        }
        if next == SIM_CONSTANTS.len() {
            break;
        }
        word = SIM_CONSTANTS[next].1;
        next += 1;
    }
    (hash ^ (hash >> 32)) as u32
}

/// What peers actually compare in [`SessionHello`]: [`PROTOCOL_VERSION`] mixed
/// with the sim constants, so two builds with different physics never play
/// together even if nobody remembered to bump the version.
pub const PROTOCOL_ID: u32 = protocol_id();

/// Longest accepted player name, in characters.
pub const MAX_NAME_CHARS: usize = 16;
/// Name used when the requested one is empty or sanitizes to nothing.
pub const DEFAULT_NAME: &str = "Player";

/// First message of every session (client -> server): who is joining, on which
/// protocol. The server only spawns a rider after accepting it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHello {
    /// The sender's [`PROTOCOL_ID`].
    pub protocol: u32,
    pub name: String,
}

/// Why the server refused a [`SessionHello`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinRejection {
    VersionMismatch {
        server: u32,
    },
    ServerFull {
        max_players: u32,
    },
    /// The server is ranked and the hello came without a join ticket.
    TicketRequired,
    /// The ticket is forged, expired, for another server or already used.
    InvalidTicket,
    /// The ticket's account already has a live session on this server.
    AlreadyConnected,
}

impl std::fmt::Display for JoinRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionMismatch { server } => write!(
                f,
                "Incompatible game version (server build {server:08x}, yours {PROTOCOL_ID:08x}). \
                 Update the client or pick another server."
            ),
            Self::ServerFull { max_players } => {
                write!(f, "Server is full ({max_players} players).")
            }
            Self::TicketRequired => write!(f, "This is a ranked server: sign in to play here."),
            Self::InvalidTicket => write!(f, "Your join ticket was not accepted. Try again."),
            Self::AlreadyConnected => write!(f, "This account is already playing on this server."),
        }
    }
}

/// Sent by the client right before its [`SessionHello`] on a ranked server:
/// the manager-signed join ticket. Ordered-reliable like the hello, so the
/// server has it when it reads the hello. A separate message keeps the hello
/// layout stable for old builds, which can then still be told they are
/// outdated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTicket {
    pub ticket: Vec<u8>,
}

/// Server -> client answer to [`SessionHello`]. `Accepted` carries the name
/// the server settled on (sanitized, de-duplicated).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelloReply {
    Accepted { name: String },
    Rejected(JoinRejection),
}

/// Invisible "format" characters (Unicode category Cf) that can reorder or hide
/// text in a name: bidi overrides and isolates, zero-width marks, joiners,
/// soft hyphen, BOM and language tags.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// Comparison form of a (sanitized) name for de-duplication: case-folded with
/// all whitespace removed, so `Neo`, `neo` and `N e o` count as the same name.
pub fn name_key(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Normalize a player-supplied name: drop control and invisible format
/// characters, turn exotic whitespace into plain spaces and collapse runs of
/// them, trim, cap the length, and fall back to [`DEFAULT_NAME`] when nothing
/// is left.
pub fn sanitize_name(raw: &str) -> String {
    let mut cleaned = String::new();
    for c in raw
        .chars()
        .filter(|c| !c.is_control() && !is_format_char(*c))
    {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c != ' ' || !cleaned.ends_with(' ') {
            cleaned.push(c);
        }
    }
    let capped: String = cleaned.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = capped.trim_end();
    if name.is_empty() {
        DEFAULT_NAME.to_owned()
    } else {
        name.to_owned()
    }
}

/// Longest accepted chat message, in characters.
pub const MAX_CHAT_CHARS: usize = 160;
/// Raw messages longer than this many bytes are rejected before any
/// processing, so a hostile peer cannot make the server scan megabytes.
const MAX_CHAT_RAW_BYTES: usize = MAX_CHAT_CHARS * 8;

/// Whether `c` may appear in a chat message: anything printable, so accents,
/// symbols and other scripts pass, but control and invisible format characters
/// (bidi overrides, zero-width marks) do not.
pub fn is_chat_char(c: char) -> bool {
    !c.is_control() && !is_format_char(c)
}

/// Normalize a chat message: drop the characters [`is_chat_char`] refuses,
/// turn exotic whitespace into plain spaces and collapse runs of them, trim and
/// cap the length. `None` when nothing is left (or the raw text is absurdly
/// long), meaning the message should be dropped.
pub fn sanitize_chat(raw: &str) -> Option<String> {
    if raw.len() > MAX_CHAT_RAW_BYTES {
        return None;
    }
    let mut cleaned = String::new();
    for c in raw.chars().filter(|c| is_chat_char(*c)) {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c != ' ' || !cleaned.ends_with(' ') {
            cleaned.push(c);
        }
    }
    let capped: String = cleaned.trim().chars().take(MAX_CHAT_CHARS).collect();
    let text = capped.trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Client -> server: a line of chat. Carries no sender; the server derives it
/// from the link, like turns and respawns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSend {
    pub text: String,
}

/// Server -> every admitted client: `name` said `text` (both sanitized).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub name: String,
    pub text: String,
}

/// Ordered-reliable channel for [`ChatSend`] and [`ChatLine`].
pub struct ChatChannel;

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

/// Client -> server, once per client tick while anything is unacknowledged:
/// every command the server has not yet reported as decided, oldest first, at
/// most [`TURN_BATCH_MAX`]. Sent on the unreliable [`TurnChannel`]; the
/// repetition, not a retransmit timer, is what makes delivery robust.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnBatch {
    pub commands: Vec<TurnCommand>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnRejectReason {
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

/// What became of one command (diagnostics; see [`TurnAck`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnVerdict {
    pub seq: u64,
    pub outcome: TurnOutcome,
}

/// Server -> client, at most once per frame, on the unreliable
/// [`TurnAckChannel`] (only the newest one matters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnAck {
    /// Every command with `seq` below this is decided (queued or rejected):
    /// the client stops repeating them. Cumulative, so a lost ack costs nothing.
    pub received_below: u64,
    /// The newest execution or rejection since the previous ack. Diagnostics
    /// only: prediction never waits for it and older verdicts may be skipped.
    pub newest: Option<TurnVerdict>,
}

/// Unreliable client -> server channel for [`TurnBatch`].
pub struct TurnChannel;
/// Unreliable, sequenced server -> client channel for [`TurnAck`].
pub struct TurnAckChannel;

/// Max buffered turns per player; beyond this new commands are rejected with
/// `QueueFull` (accepted commands are never silently dropped).
pub const TURN_QUEUE_CAP: usize = 32;
/// Most commands in one [`TurnBatch`]; also the client's outbox size.
pub const TURN_BATCH_MAX: usize = 16;
/// The client stops repeating a command this many ticks after its stamp
/// (400 ms): by then the server would reject it as too late anyway.
pub const TURN_RESEND_TICKS: u32 = 48;
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
    // Turns are repeated by the sender in every packet, so neither direction
    // needs a reliable channel (and the server never buffers for a client that
    // does not acknowledge). `Unordered`: each batch is a superset of the
    // older ones, and the server dedupes by seq.
    app.add_channel::<TurnChannel>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliable,
        ..Default::default()
    })
    .add_direction(NetworkDirection::ClientToServer);
    app.add_channel::<TurnAckChannel>(ChannelSettings {
        mode: ChannelMode::SequencedUnreliable,
        ..Default::default()
    })
    .add_direction(NetworkDirection::ServerToClient);
    app.register_message::<TurnBatch>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<TurnAck>()
        .add_direction(NetworkDirection::ServerToClient);
    // Registered last on purpose: appending keeps every earlier message id
    // stable, so an older peer still decodes the hello pair.
    app.register_message::<SessionTicket>()
        .add_direction(NetworkDirection::ClientToServer);
    app.add_channel::<ChatChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    app.register_message::<ChatSend>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<ChatLine>()
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
    // Render-only smoothing: the head slides along the trail between snapshots.
    // Collision reads `ConfirmedHistory<Trail>`, not this interpolated value.
    app.component::<Trail>()
        .replicate()
        .predict()
        .add_interpolation_with(interpolate_trail_component);

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

fn interpolate_trail_component(start: Trail, end: Trail, t: f32) -> Trail {
    Trail(crate::polyline::interpolate_trail(&start.0, &end.0, t))
}

// Discrete snapshots keep cardinal turns on the tick they happened.
fn step<T>(start: T, end: T, t: f32) -> T {
    if t >= 1.0 { end } else { start }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_invisible_and_bidi_characters() {
        // Right-to-left override, zero-width space, BOM, soft hyphen.
        assert_eq!(sanitize_name("\u{202E}Neo\u{200B}\u{FEFF}\u{AD}"), "Neo");
        assert_eq!(sanitize_name("a\u{2066}b\u{2069}"), "ab");
        assert_eq!(sanitize_name("\u{200B}\u{202E}"), DEFAULT_NAME);
        // Exotic spaces become one plain space.
        assert_eq!(sanitize_name("a\u{00A0}\u{3000} b"), "a b");
    }

    #[test]
    fn sanitize_chat_cleans_and_drops_empty() {
        assert_eq!(
            sanitize_chat("  héllo \u{202E}wörld\n"),
            Some("héllo wörld".into())
        );
        assert_eq!(sanitize_chat("\u{200B}\t \u{7}"), None);
        assert_eq!(sanitize_chat(""), None);
        let long = "a".repeat(MAX_CHAT_CHARS + 50);
        assert_eq!(
            sanitize_chat(&long).unwrap().chars().count(),
            MAX_CHAT_CHARS
        );
        assert_eq!(sanitize_chat(&"a".repeat(MAX_CHAT_RAW_BYTES + 1)), None);
    }

    #[test]
    fn name_key_ignores_case_and_spacing() {
        assert_eq!(name_key("Neo"), name_key("nEo"));
        assert_eq!(name_key("N e o"), name_key("neo"));
        assert_ne!(name_key("Neo 2"), name_key("Neo"));
    }

    #[test]
    fn every_constant_is_part_of_the_protocol_id() {
        let source = include_str!("constants.rs");
        let declared: Vec<&str> = source
            .lines()
            .filter_map(|line| line.strip_prefix("pub const "))
            .filter_map(|rest| rest.split(':').next())
            .collect();
        assert!(declared.len() > 10, "constants.rs was not parsed");
        for name in declared {
            assert!(
                SIM_CONSTANTS.iter().any(|(listed, _)| *listed == name),
                "{name} is not in SIM_CONSTANTS: add it so a retune changes PROTOCOL_ID"
            );
        }
    }

    #[test]
    fn protocol_id_depends_on_the_sim_constants() {
        assert_ne!(PROTOCOL_ID, PROTOCOL_VERSION);
        assert!(SIM_CONSTANTS.iter().all(|(_, bits)| *bits != 0));
    }

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
