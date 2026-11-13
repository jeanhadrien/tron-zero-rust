//! Connection lifecycle independent of menu presentation and server discovery.
//!
//! The lifecycle is an explicit state machine ([`State`]): every phase carries
//! only the data that is valid in it, and each teardown system acts in exactly
//! one phase, so no system ordering can skip a step:
//!
//! ```text
//! Dialing -> Handshaking -> Synchronizing <-> Playing
//!    |            \               |              /
//!    |             `----- disconnect() --------'
//!    |                        v
//!    |                    Leaving --send_leave--> Unlinking --unlink_session--> Draining
//!    `---- disconnect() (nothing sent yet) -------^                                |
//!                                                        Offline <--finish_disconnect
//! ```

use std::net::SocketAddr;

use bevy::prelude::*;
use lightyear::prelude::client::{Connect, Disconnect, InputDelayConfig, RawClient};
use lightyear::prelude::input::native::{InputMarker, NativeBuffer};
use lightyear::prelude::*;
use shared::protocol::{
    HelloReply, JoinRejection, PROTOCOL_ID, SESSION_HEARTBEAT_SECS, SESSION_TIMEOUT_SECS,
    SessionChannel, SessionHello, SessionReply, SessionRequest, SessionTicket, sanitize_name,
};

use crate::input::{InputLifecycle, PendingInput, RespawnUi, TurnAckState, TurnOutbox, TurnSeq};
use crate::menu::MenuState;
use crate::settings::PlayerName;

const CONNECT_TIMEOUT_SECS: f64 = 10.0;
/// Longest a quit waits for the leave to flush before exiting anyway.
const QUIT_GRACE_SECS: f64 = 2.0;
pub const IDLE_STATUS: &str = "Not connected. Browse servers or start a local one.";

/// What the rest of the client needs to know about the connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionPhase {
    #[default]
    Offline,
    Connecting,
    Synchronizing,
    Playing,
    Disconnecting,
}

/// The live transport link to one server.
#[derive(Debug)]
struct Link {
    client: Entity,
    address: SocketAddr,
    /// When the last heartbeat went out.
    last_send: Option<f64>,
    /// The name the server admitted us under. Admission can arrive before or
    /// after the first heartbeat reply, so it lives on the link, not a phase.
    admitted_name: Option<String>,
}

/// The connection state machine. `since` is the start of the connect deadline.
#[derive(Debug)]
enum State {
    Offline {
        /// Why the server refused the last join, kept until the next connect
        /// so automatic reconnecting can tell a refusal from a dropped link.
        refusal: Option<JoinRejection>,
    },
    /// Link requested, hello not sent yet. The only phase holding a join ticket.
    Dialing {
        link: Link,
        since: f64,
        ticket: Option<Vec<u8>>,
    },
    /// Hello sent, no heartbeat reply yet.
    Handshaking {
        link: Link,
        since: f64,
    },
    /// The server answers; waiting for admission, clock, arena and rider.
    Synchronizing {
        link: Link,
        since: f64,
        last_reply: f64,
    },
    Playing {
        link: Link,
        last_reply: f64,
    },
    /// Disconnect requested: [`send_leave`] queues the Leave.
    Leaving {
        link: Link,
        refusal: Option<JoinRejection>,
    },
    /// Leave queued (or nothing to leave): [`unlink_session`] closes the link.
    Unlinking {
        entity: Entity,
        refusal: Option<JoinRejection>,
    },
    /// Link closing: [`finish_disconnect`] waits for it and the replicas to go.
    Draining {
        entity: Entity,
        refusal: Option<JoinRejection>,
    },
}

impl Default for State {
    fn default() -> Self {
        Self::Offline { refusal: None }
    }
}

impl State {
    fn link(&self) -> Option<&Link> {
        match self {
            Self::Dialing { link, .. }
            | Self::Handshaking { link, .. }
            | Self::Synchronizing { link, .. }
            | Self::Playing { link, .. }
            | Self::Leaving { link, .. } => Some(link),
            _ => None,
        }
    }

    fn link_mut(&mut self) -> Option<&mut Link> {
        match self {
            Self::Dialing { link, .. }
            | Self::Handshaking { link, .. }
            | Self::Synchronizing { link, .. }
            | Self::Playing { link, .. }
            | Self::Leaving { link, .. } => Some(link),
            _ => None,
        }
    }

    fn refusal(&self) -> Option<JoinRejection> {
        match self {
            Self::Offline { refusal }
            | Self::Leaving { refusal, .. }
            | Self::Unlinking { refusal, .. }
            | Self::Draining { refusal, .. } => *refusal,
            _ => None,
        }
    }
}

#[derive(Resource)]
pub struct Session {
    pub status: String,
    /// Protocol advertised in the hello (a field so tests can fake a stale build).
    pub protocol: u32,
    /// Name requested in the hello; see [`Session::name`] for the admitted one.
    name: String,
    /// Set by Quit: exit once the session is torn down (or the grace expires).
    /// Survives `connect`, and once set no new connection may start.
    quit_requested_at: Option<f64>,
    state: State,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            status: IDLE_STATUS.into(),
            protocol: PROTOCOL_ID,
            name: shared::protocol::DEFAULT_NAME.into(),
            quit_requested_at: None,
            state: State::default(),
        }
    }
}

/// Where to dial a server: its address plus, for WebTransport, the SHA-256
/// digest (lowercase hex) of the self-signed certificate the browser pins.
/// Native UDP ignores the digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub addr: SocketAddr,
    pub cert_digest: Option<String>,
}

impl From<SocketAddr> for Endpoint {
    fn from(addr: SocketAddr) -> Self {
        Self {
            addr,
            cert_digest: None,
        }
    }
}

/// The link's IO component: raw UDP natively, WebTransport in the browser.
#[cfg(not(target_family = "wasm"))]
fn transport_io(_cert_digest: Option<String>) -> impl Bundle {
    (
        UdpIo::default(),
        LocalAddr(SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0))),
    )
}

/// The browser trusts the server's self-signed certificate by its SHA-256
/// digest (hex); an empty digest falls back to CA validation.
#[cfg(target_family = "wasm")]
fn transport_io(cert_digest: Option<String>) -> impl Bundle {
    (
        lightyear::webtransport::client::WebTransportClientIo {
            certificate_digest: cert_digest.unwrap_or_default(),
        },
        // RawClient reads LocalAddr when the link comes up; WebTransport only
        // sets it later, so a placeholder must exist from the start.
        LocalAddr(SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0))),
    )
}

impl Session {
    /// Start connecting to `endpoint` (a bare `SocketAddr` works too) over the
    /// build's transport: UDP natively, WebTransport in the browser. Joins as
    /// `name` (the server has the final say on it). Does nothing while a
    /// session exists or a quit is pending.
    pub fn connect(
        &mut self,
        endpoint: impl Into<Endpoint>,
        name: &str,
        now: f64,
        commands: &mut Commands,
    ) {
        self.dial(endpoint.into(), name, None, now, commands);
    }

    /// [`Session::connect`] to a ranked server: `ticket` goes out right before
    /// the hello. Tickets are single-use, so a reconnect needs a fresh one.
    pub fn connect_with_ticket(
        &mut self,
        endpoint: impl Into<Endpoint>,
        name: &str,
        ticket: Vec<u8>,
        now: f64,
        commands: &mut Commands,
    ) {
        self.dial(endpoint.into(), name, Some(ticket), now, commands);
    }

    fn dial(
        &mut self,
        endpoint: Endpoint,
        name: &str,
        ticket: Option<Vec<u8>>,
        now: f64,
        commands: &mut Commands,
    ) {
        let Endpoint {
            addr: address,
            cert_digest,
        } = endpoint;
        self.start(
            address,
            (transport_io(cert_digest), PeerAddr(address)),
            name,
            ticket,
            now,
            commands,
        );
    }

    /// [`Session::connect`] over another transport: `io` is the link's IO
    /// bundle (it must include `PeerAddr(address)`), e.g. WebTransport.
    pub fn connect_with(
        &mut self,
        address: SocketAddr,
        io: impl Bundle,
        name: &str,
        now: f64,
        commands: &mut Commands,
    ) {
        self.start(address, io, name, None, now, commands);
    }

    /// Whether a new connection may start: fully offline and no quit pending.
    fn can_start(&self) -> bool {
        matches!(self.state, State::Offline { .. }) && self.quit_requested_at.is_none()
    }

    /// Spawn the link and enter `Dialing`. The ticket is only kept if this
    /// call really starts a connection.
    fn start(
        &mut self,
        address: SocketAddr,
        io: impl Bundle,
        name: &str,
        ticket: Option<Vec<u8>>,
        now: f64,
        commands: &mut Commands,
    ) {
        if !self.can_start() {
            return;
        }
        let client = commands
            .spawn((
                RawClient,
                io,
                PredictionManager::default(),
                InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
            ))
            .id();
        self.status = format!("Connecting to {address}...");
        self.name = name.to_owned();
        self.state = State::Dialing {
            link: Link {
                client,
                address,
                last_send: None,
                admitted_name: None,
            },
            since: now,
            ticket,
        };
        info!(?client, %address, "Connecting to server");
        // Connect already triggers LinkStart in Lightyear.
        commands.trigger(Connect { entity: client });
    }

    pub fn phase(&self) -> ConnectionPhase {
        match self.state {
            State::Offline { .. } => ConnectionPhase::Offline,
            State::Dialing { .. } | State::Handshaking { .. } => ConnectionPhase::Connecting,
            State::Synchronizing { .. } => ConnectionPhase::Synchronizing,
            State::Playing { .. } => ConnectionPhase::Playing,
            State::Leaving { .. } | State::Unlinking { .. } | State::Draining { .. } => {
                ConnectionPhase::Disconnecting
            }
        }
    }

    /// The player's name: the server's final choice while a link exists and has
    /// been admitted, else the one requested. Once teardown starts
    /// (`Unlinking`, `Draining`, `Offline`) the link data is gone, so this falls
    /// back to the requested name.
    pub fn name(&self) -> &str {
        self.state
            .link()
            .and_then(|link| link.admitted_name.as_deref())
            .unwrap_or(&self.name)
    }

    /// Begin tearing the session down. A session that never sent its hello
    /// has nothing to leave and goes straight to unlinking; any later one
    /// goes through `Leaving`, so the server is always told, whatever order
    /// the teardown systems run in.
    pub fn disconnect(&mut self, status: impl Into<String>) {
        self.leave(status.into(), None);
    }

    /// [`Session::disconnect`] because the server refused the hello; the
    /// refusal outlives the teardown (see [`Session::join_rejected`]).
    fn disconnect_refused(&mut self, reason: JoinRejection) {
        self.leave(reason.to_string(), Some(reason));
    }

    fn leave(&mut self, status: String, refusal: Option<JoinRejection>) {
        self.state = match std::mem::take(&mut self.state) {
            State::Dialing { link, .. } => State::Unlinking {
                entity: link.client,
                refusal,
            },
            State::Handshaking { link, .. }
            | State::Synchronizing { link, .. }
            | State::Playing { link, .. } => State::Leaving { link, refusal },
            // Offline or already tearing down: keep the teardown going, but
            // do not lose a refusal that arrives late (it stops auto-connect).
            mut other => {
                if let State::Leaving { refusal: slot, .. }
                | State::Unlinking { refusal: slot, .. }
                | State::Draining { refusal: slot, .. } = &mut other
                {
                    *slot = slot.or(refusal);
                }
                self.state = other;
                return;
            }
        };
        self.status = status;
        info!(reason = %self.status, "Disconnecting session");
    }

    /// Leave the server (if connected) and exit the app once cleanup is done.
    pub fn request_quit(&mut self, now: f64) {
        self.quit_requested_at.get_or_insert(now);
        self.disconnect("Quitting...");
    }

    pub fn quit_requested(&self) -> bool {
        self.quit_requested_at.is_some()
    }

    /// Whether the server refused this session's hello (version, capacity).
    pub fn join_rejected(&self) -> bool {
        self.state.refusal().is_some()
    }

    /// The client entity while the link is live (the phases `monitor_connection` watches).
    fn live_client(&self) -> Option<Entity> {
        self.state
            .link()
            .filter(|_| !matches!(self.state, State::Leaving { .. }))
            .map(|link| link.client)
    }

    /// The client entity once the hello has been queued on it (any phase from
    /// `Handshaking` to `Playing`); what `send_look` keys on.
    pub(crate) fn hello_link(&self) -> Option<Entity> {
        match &self.state {
            State::Handshaking { link, .. }
            | State::Synchronizing { link, .. }
            | State::Playing { link, .. } => Some(link.client),
            _ => None,
        }
    }

    fn is_dialing(&self) -> bool {
        matches!(self.state, State::Dialing { .. })
    }

    /// Whether a heartbeat should go out at `now`: one per interval, and never
    /// while dialing, since a heartbeat ahead of the hello would bind the
    /// server's nonce for a link that then has no Leave to send (see
    /// [`Session::leave`]).
    fn heartbeat_due(&self, now: f64) -> bool {
        !self.is_dialing()
            && self.state.link().is_some_and(|link| {
                link.last_send
                    .is_none_or(|last| now - last >= SESSION_HEARTBEAT_SECS)
            })
    }

    fn heartbeat_sent(&mut self, now: f64) {
        if let Some(link) = self.state.link_mut() {
            link.last_send = Some(now);
        }
    }

    /// Whether the session is past the handshake but not yet (or no longer)
    /// proven ready, so readiness must be re-checked every frame.
    fn awaits_readiness(&self) -> bool {
        matches!(
            self.phase(),
            ConnectionPhase::Synchronizing | ConnectionPhase::Playing
        )
    }

    /// Dialing -> Handshaking once the hello is queued; hands out the join
    /// ticket, which goes out right before the hello and is never kept after.
    fn hello_sent(&mut self) -> Option<Vec<u8>> {
        match std::mem::take(&mut self.state) {
            State::Dialing {
                link,
                since,
                ticket,
            } => {
                self.state = State::Handshaking { link, since };
                ticket
            }
            other => {
                self.state = other;
                None
            }
        }
    }

    /// The server accepted the hello and chose `name`.
    fn admit(&mut self, name: String) {
        if let Some(link) = self.state.link_mut() {
            link.admitted_name = Some(name);
        }
    }

    fn admitted(&self) -> bool {
        self.state
            .link()
            .is_some_and(|link| link.admitted_name.is_some())
    }

    /// A heartbeat reply arrived: the first one leaves `Handshaking`.
    fn note_reply(&mut self, now: f64) {
        self.state = match std::mem::take(&mut self.state) {
            State::Handshaking { link, since } => {
                info!(entity = ?link.client, "Server heartbeat confirmed");
                State::Synchronizing {
                    link,
                    since,
                    last_reply: now,
                }
            }
            State::Synchronizing { link, since, .. } => State::Synchronizing {
                link,
                since,
                last_reply: now,
            },
            State::Playing { link, .. } => State::Playing {
                link,
                last_reply: now,
            },
            other => other,
        };
    }

    /// Synchronizing -> Playing. Returns whether the phase changed.
    fn enter_playing(&mut self) -> bool {
        match std::mem::take(&mut self.state) {
            State::Synchronizing {
                link, last_reply, ..
            } => {
                self.state = State::Playing { link, last_reply };
                true
            }
            other => {
                self.state = other;
                false
            }
        }
    }

    /// Playing -> Synchronizing after a hiccup, with a fresh connect deadline
    /// (the old one started long ago and would expire at once).
    fn lose_sync(&mut self, now: f64) {
        self.state = match std::mem::take(&mut self.state) {
            State::Playing { link, last_reply } => State::Synchronizing {
                link,
                since: now,
                last_reply,
            },
            other => other,
        };
    }

    /// Why the session should be given up on at `now`, if it should.
    fn timed_out(&self, now: f64) -> Option<String> {
        let (link, since, last_reply) = match &self.state {
            State::Dialing { link, since, .. } | State::Handshaking { link, since } => {
                (link, Some(*since), None)
            }
            State::Synchronizing {
                link,
                since,
                last_reply,
            } => (link, Some(*since), Some(*last_reply)),
            State::Playing { link, last_reply } => (link, None, Some(*last_reply)),
            _ => return None,
        };
        let target = link.address;
        if last_reply.is_some_and(|last| now - last >= SESSION_TIMEOUT_SECS) {
            Some(format!(
                "Lost contact with {target}. Check the server and retry."
            ))
        } else if since.is_some_and(|since| now - since >= CONNECT_TIMEOUT_SECS) {
            Some(format!(
                "No response from {target} (timed out). Is the server running and reachable?"
            ))
        } else {
            None
        }
    }

    /// Test fixture: jump straight to `phase` with a placeholder link.
    #[cfg(test)]
    pub fn force_phase(&mut self, phase: ConnectionPhase) {
        self.state = match phase {
            ConnectionPhase::Offline => State::default(),
            ConnectionPhase::Connecting => State::Dialing {
                link: tests::link(Entity::PLACEHOLDER),
                since: 0.0,
                ticket: None,
            },
            ConnectionPhase::Synchronizing => State::Synchronizing {
                link: tests::link(Entity::PLACEHOLDER),
                since: 0.0,
                last_reply: 0.0,
            },
            ConnectionPhase::Playing => State::Playing {
                link: tests::link(Entity::PLACEHOLDER),
                last_reply: 0.0,
            },
            ConnectionPhase::Disconnecting => State::Unlinking {
                entity: Entity::PLACEHOLDER,
                refusal: None,
            },
        };
    }

    /// Test fixture: the join ticket held while dialing.
    #[cfg(test)]
    fn ticket(&self) -> Option<&[u8]> {
        match &self.state {
            State::Dialing { ticket, .. } => ticket.as_deref(),
            _ => None,
        }
    }
}

/// Registers the connection lifecycle systems and their ordering. The real
/// client and the nettest client both call this, so the ordering constraints
/// live (and are tested) in one place. Each caller adds its own UI pass
/// (`menu_controls`, `chat_controls`) `.after(monitor_connection)` and
/// `.before(input::buffer_keyboard_input)`.
pub fn add_lifecycle_systems(app: &mut App) {
    // `Look` keeps a value inserted earlier (the configured one).
    app.init_resource::<crate::look::Look>();
    app.init_resource::<crate::look::LookSync>();
    // Frame start: raw input is read, the monitor then sees this frame's
    // network messages and replicated entities.
    app.add_systems(
        PreUpdate,
        (
            crate::menu::begin_input_frame,
            crate::attach_local_input,
            monitor_connection,
            // Right after the hello is queued: SetLook follows it on the same
            // ordered channel.
            crate::look::send_look,
        )
            .chain()
            .after(bevy::input::InputSystems)
            .after(MessageSystems::Receive)
            .after(ReplicationSystems::Receive)
            .before(crate::input::buffer_keyboard_input),
    );
    app.add_systems(Update, (send_leave, finish_disconnect));
    // The UDP client flushes `Link::send` in PreUpdate (not PostUpdate), so the
    // Leave queued in Update only reaches the socket on the next frame: close
    // the link after that flush, or the Leave is lost.
    app.add_systems(PreUpdate, unlink_session.after(LinkSystems::Send));
}

pub fn monitor_connection(
    mut session: ResMut<Session>,
    mut menu: ResMut<MenuState>,
    time: Res<Time<Real>>,
    mut clients: Query<
        (
            Has<Connected>,
            Has<IsSynced<InputTimeline>>,
            Option<&Disconnected>,
            Option<&mut MessageReceiver<SessionReply>>,
            Option<&mut MessageSender<SessionRequest>>,
            Option<&mut MessageReceiver<HelloReply>>,
            Option<&mut MessageSender<SessionHello>>,
            Option<&mut MessageSender<SessionTicket>>,
        ),
        With<Client>,
    >,
    players: Query<
        Option<&ControlledBy>,
        (
            With<shared::Player>,
            With<Controlled>,
            With<Predicted>,
            With<InputMarker<shared::PlayerInput>>,
            With<NativeBuffer<shared::PlayerInput>>,
            With<shared::LifeGeneration>,
        ),
    >,
    arenas: Query<(), (With<shared::Arena>, With<shared::WallSegments>)>,
) {
    let Some(entity) = session.live_client() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let Ok((
        connected,
        synced,
        disconnected,
        receiver,
        sender,
        hello_receiver,
        hello_sender,
        ticket_sender,
    )) = clients.get_mut(entity)
    else {
        session.disconnect("Connection entity was lost. Please retry.");
        menu.open = true;
        return;
    };
    if let Some(reason) = disconnected.and_then(|state| state.reason.as_deref()) {
        warn!(%reason, "Connection failed");
        session.disconnect(format!("Connection failed: {reason}"));
        menu.open = true;
        return;
    }
    if connected
        && session.is_dialing()
        && let Some(mut hello_sender) = hello_sender
    {
        // Same ordered channel as the heartbeat below, so the server sees the
        // hello first, and the ticket (if any) before the hello.
        match (session.hello_sent(), ticket_sender) {
            (Some(ticket), Some(mut ticket_sender)) => {
                ticket_sender.send::<SessionChannel>(SessionTicket { ticket });
            }
            (Some(_), None) => warn!("No ticket sender on the link; joining without a ticket"),
            (None, _) => {}
        }
        hello_sender.send::<SessionChannel>(SessionHello {
            protocol: session.protocol,
            name: session.name.clone(),
        });
    }
    if let Some(mut hello_receiver) = hello_receiver {
        for reply in hello_receiver.receive() {
            match reply {
                HelloReply::Accepted { name } => {
                    info!(%name, "Join accepted");
                    session.admit(name);
                }
                HelloReply::Rejected(reason) => {
                    warn!(%reason, "Join rejected");
                    session.disconnect_refused(reason);
                    menu.open = true;
                    return;
                }
            }
        }
    }
    if let Some(mut receiver) = receiver {
        for reply in receiver.receive() {
            if reply.nonce == entity.to_bits() {
                session.note_reply(now);
            }
        }
    }
    if let Some(reason) = session.timed_out(now) {
        warn!(%reason);
        session.disconnect(reason);
        menu.open = true;
        return;
    }
    if connected
        && session.heartbeat_due(now)
        && let Some(mut sender) = sender
    {
        sender.send::<SessionChannel>(SessionRequest::Heartbeat {
            nonce: entity.to_bits(),
        });
        session.heartbeat_sent(now);
    }
    if session.awaits_readiness() {
        // Controlled is replicated; ControlledBy is normally sender-side only.
        let rider = players
            .iter()
            .any(|owner| owns_rider(owner.map(|c| c.owner), entity));
        let readiness = Readiness {
            connected,
            synced,
            admitted: session.admitted(),
            arena: !arenas.is_empty(),
            rider,
        };
        match readiness.pending_status() {
            None => {
                if session.enter_playing() {
                    info!(
                        ?entity,
                        "Session ready: clock, arena and controlled rider received"
                    );
                    session.status = format!("Connected as {}.", session.name());
                    menu.open = false;
                    menu.block_this_frame = true;
                }
            }
            Some(status) => {
                session.lose_sync(now);
                if session.status != status {
                    info!(?entity, %status, "Session readiness pending");
                    session.status = status.into();
                }
                menu.open = true;
            }
        }
    }
}

/// Everything that must hold before gameplay starts, gathered from the world.
#[derive(Clone, Copy, Debug)]
struct Readiness {
    connected: bool,
    /// The input clock is synchronized.
    synced: bool,
    /// The server accepted the hello.
    admitted: bool,
    /// The arena and its walls have replicated.
    arena: bool,
    /// The controlled rider (with input buffer) is present.
    rider: bool,
}

impl Readiness {
    /// What we are still waiting for, or `None` when ready to play. The first
    /// missing piece wins, in the order the server delivers them.
    fn pending_status(&self) -> Option<&'static str> {
        Some(if !self.connected {
            "Server answered. Waiting for connection..."
        } else if !self.admitted {
            "Server answered. Waiting for the join to be accepted..."
        } else if !self.synced {
            "Server answered. Synchronizing clock..."
        } else if !self.arena {
            "Clock synchronized. Waiting for arena..."
        } else if !self.rider {
            "Clock synchronized. Waiting for controlled rider and input buffer..."
        } else {
            return None;
        })
    }
}

/// Whether a rider belongs to `me`. `ControlledBy` is normally sender-side
/// only, so a replicated rider without it counts as ours.
fn owns_rider(owner: Option<Entity>, me: Entity) -> bool {
    owner.is_none_or(|owner| owner == me)
}

/// `Leaving`: queue the Leave before the transport sends. Always moves on to
/// `Unlinking`, even if the sender is gone, so a teardown can never hang.
pub fn send_leave(
    mut session: ResMut<Session>,
    mut senders: Query<&mut MessageSender<SessionRequest>>,
) {
    let State::Leaving { link, refusal } = &session.state else {
        return;
    };
    let (entity, refusal) = (link.client, *refusal);
    if let Ok(mut sender) = senders.get_mut(entity) {
        sender.send::<SessionChannel>(SessionRequest::Leave {
            nonce: entity.to_bits(),
        });
    } else {
        warn!(?entity, "No sender to queue the leave on");
    }
    session.state = State::Unlinking { entity, refusal };
}

/// `Unlinking`: close the socket once the Leave has been sent. Schedule it in
/// PreUpdate after `LinkSystems::Send`: the UDP client only flushes its link
/// buffer there, one frame after `send_leave` queued the Leave.
pub fn unlink_session(mut session: ResMut<Session>, mut commands: Commands) {
    let State::Unlinking { entity, refusal } = session.state else {
        return;
    };
    // The entity can be gone (it was lost, or never spawned); triggering on it would panic.
    if commands.get_entity(entity).is_ok() {
        commands.trigger(Disconnect { entity });
    }
    session.state = State::Draining { entity, refusal };
}

/// Dev convenience (`TRON_AUTOCONNECT=host:port`): keep trying to join a
/// server from startup until it works or the player takes over.
#[derive(Resource, Debug)]
pub struct AutoConnect {
    endpoint: Endpoint,
    armed: bool,
    next_attempt: f64,
}

/// Pause between auto-connect attempts, so a server that is still starting
/// up is picked up without hammering it.
const AUTOCONNECT_RETRY_SECS: f64 = 1.5;

impl AutoConnect {
    /// Read `TRON_AUTOCONNECT`; an unparsable value is logged and ignored.
    pub fn from_env() -> Option<Self> {
        let raw = crate::platform::env_var("TRON_AUTOCONNECT")?;
        match raw.trim().parse::<SocketAddr>() {
            Ok(addr) => {
                info!(%addr, "TRON_AUTOCONNECT set: joining automatically");
                // Browsers pin the dev server's certificate by digest.
                let cert_digest = crate::platform::env_var("TRON_CERT_DIGEST")
                    .map(|digest| digest.trim().to_ascii_lowercase())
                    .filter(|digest| !digest.is_empty());
                Some(Self {
                    endpoint: Endpoint { addr, cert_digest },
                    armed: true,
                    next_attempt: 0.0,
                })
            }
            Err(err) => {
                warn!(%raw, %err, "Ignoring invalid TRON_AUTOCONNECT (expected host:port)");
                None
            }
        }
    }

    /// Stop auto-connecting (the player cancelled, disconnected or quit).
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

/// Join the autoconnect target while armed and idle. Disarms itself once a
/// session reaches Playing (so a later disconnect stays disconnected), when the
/// server refuses the join (retrying cannot help and would wipe the reason from
/// the menu), and as soon as a quit is requested.
pub fn auto_connect(
    auto: Option<ResMut<AutoConnect>>,
    mut session: ResMut<Session>,
    name: Res<PlayerName>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let Some(mut auto) = auto.filter(|auto| auto.armed) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if session.phase() == ConnectionPhase::Playing
        || session.join_rejected()
        || session.quit_requested()
    {
        auto.disarm();
    } else if session.phase() == ConnectionPhase::Offline && now >= auto.next_attempt {
        auto.next_attempt = now + AUTOCONNECT_RETRY_SECS;
        session.connect(
            auto.endpoint.clone(),
            &sanitize_name(&name.0),
            now,
            &mut commands,
        );
    }
}

/// Exit the app once a requested quit has finished tearing the session down
/// (so the server gets the leave), or after a short grace if it cannot.
pub fn quit_when_ready(
    session: Res<Session>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(requested_at) = session.quit_requested_at else {
        return;
    };
    if session.phase() == ConnectionPhase::Offline
        || time.elapsed_secs_f64() - requested_at >= QUIT_GRACE_SECS
    {
        exit.write(AppExit::Success);
    }
}

/// `Draining`: wait for the Lightyear state-transition cleanup before allowing
/// another link.
pub fn finish_disconnect(
    mut session: ResMut<Session>,
    clients: Query<Has<Disconnected>, With<Client>>,
    replicated: Query<(), With<Replicated>>,
    mut pending: ResMut<PendingInput>,
    mut lifecycle: ResMut<InputLifecycle>,
    mut respawn: ResMut<RespawnUi>,
    mut turn_seq: ResMut<TurnSeq>,
    mut turn_acks: ResMut<TurnAckState>,
    mut outbox: ResMut<TurnOutbox>,
    mut commands: Commands,
) {
    let State::Draining { entity, refusal } = session.state else {
        return;
    };
    let disconnected = clients.get(entity).unwrap_or(true);
    if !disconnected || !replicated.is_empty() {
        return;
    }
    if let Ok(mut client) = commands.get_entity(entity) {
        client.despawn();
    }
    pending.0.clear();
    *lifecycle = InputLifecycle::default();
    *respawn = RespawnUi::default();
    *turn_seq = TurnSeq::default();
    *turn_acks = TurnAckState::default();
    *outbox = TurnOutbox::default();
    session.state = State::Offline { refusal };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keyboard::PendingPress;
    use bevy::ecs::system::RunSystemOnce;

    const ADDRESS: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 9);

    pub(super) fn link(client: Entity) -> Link {
        Link {
            client,
            address: ADDRESS,
            last_send: None,
            admitted_name: None,
        }
    }

    fn session_in(state: State) -> Session {
        Session {
            state,
            ..Default::default()
        }
    }

    fn playing(client: Entity) -> State {
        State::Playing {
            link: link(client),
            last_reply: 0.0,
        }
    }

    /// A world with everything the teardown systems touch.
    fn teardown_world(session: Session) -> World {
        let mut world = World::new();
        world.insert_resource(session);
        world.init_resource::<PendingInput>();
        world.init_resource::<InputLifecycle>();
        world.init_resource::<RespawnUi>();
        world.init_resource::<TurnSeq>();
        world.init_resource::<TurnAckState>();
        world.init_resource::<TurnOutbox>();
        world
    }

    #[test]
    fn timeout_distinguishes_no_server_from_lost_session() {
        let mut session = session_in(State::Dialing {
            link: link(Entity::PLACEHOLDER),
            since: 0.0,
            ticket: None,
        });
        assert!(session.timed_out(9.99).is_none());
        assert!(session.timed_out(10.0).unwrap().contains("timed out"));
        session.state = State::Playing {
            link: link(Entity::PLACEHOLDER),
            last_reply: 20.0,
        };
        assert!(session.timed_out(24.99).is_none());
        assert!(session.timed_out(25.0).unwrap().contains("Lost contact"));
        session.state = State::default();
        assert!(session.timed_out(1000.0).is_none(), "nothing to time out");
    }

    #[test]
    fn readiness_reports_the_first_missing_piece() {
        let ready = Readiness {
            connected: true,
            synced: true,
            admitted: true,
            arena: true,
            rider: true,
        };
        assert_eq!(ready.pending_status(), None);
        let cases = [
            (
                Readiness {
                    connected: false,
                    ..ready
                },
                "Waiting for connection",
            ),
            (
                Readiness {
                    admitted: false,
                    ..ready
                },
                "Waiting for the join to be accepted",
            ),
            (
                Readiness {
                    synced: false,
                    ..ready
                },
                "Synchronizing clock",
            ),
            (
                Readiness {
                    arena: false,
                    ..ready
                },
                "Waiting for arena",
            ),
            (
                Readiness {
                    rider: false,
                    ..ready
                },
                "Waiting for controlled rider",
            ),
        ];
        for (readiness, expected) in cases {
            let status = readiness.pending_status().unwrap();
            assert!(status.contains(expected), "{status:?} vs {expected:?}");
        }
        // Several missing pieces: the earliest in delivery order is reported.
        let nothing = Readiness {
            connected: true,
            synced: false,
            admitted: false,
            arena: false,
            rider: false,
        };
        assert!(nothing.pending_status().unwrap().contains("join"));
    }

    #[test]
    fn a_rider_without_an_owner_component_counts_as_ours() {
        let (me, other) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        assert!(owns_rider(None, me));
        assert!(owns_rider(Some(me), me));
        assert!(!owns_rider(Some(other), me));
    }

    #[test]
    fn heartbeats_are_paced_and_never_precede_the_hello() {
        let handshaking = |last_send| {
            let mut link = link(Entity::PLACEHOLDER);
            link.last_send = last_send;
            session_in(State::Handshaking { link, since: 0.0 })
        };
        assert!(
            handshaking(None).heartbeat_due(0.0),
            "the first is due at once"
        );
        assert!(!handshaking(Some(1.0)).heartbeat_due(1.0 + SESSION_HEARTBEAT_SECS - 0.01));
        assert!(handshaking(Some(1.0)).heartbeat_due(1.0 + SESSION_HEARTBEAT_SECS));
        // Dialing: never, even with nothing sent yet.
        let dialing = session_in(State::Dialing {
            link: link(Entity::PLACEHOLDER),
            since: 0.0,
            ticket: None,
        });
        assert!(!dialing.heartbeat_due(1000.0));
        // No link at all: nothing to send.
        assert!(!session_in(State::default()).heartbeat_due(1000.0));
        // Recording a send restarts the interval.
        let mut session = handshaking(None);
        session.heartbeat_sent(5.0);
        assert!(!session.heartbeat_due(5.0));
        assert!(session.heartbeat_due(5.0 + SESSION_HEARTBEAT_SECS));
    }

    #[test]
    fn readiness_is_only_checked_after_the_handshake() {
        assert!(!session_in(State::default()).awaits_readiness());
        for phase in [ConnectionPhase::Connecting, ConnectionPhase::Disconnecting] {
            let mut session = Session::default();
            session.force_phase(phase);
            assert!(!session.awaits_readiness());
        }
        for phase in [ConnectionPhase::Synchronizing, ConnectionPhase::Playing] {
            let mut session = Session::default();
            session.force_phase(phase);
            assert!(session.awaits_readiness());
        }
    }

    fn auto_world(session: Session) -> World {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.insert_resource(PlayerName::default());
        world.insert_resource(session);
        world.insert_resource(AutoConnect {
            endpoint: ADDRESS.into(),
            armed: true,
            next_attempt: 0.0,
        });
        world
    }

    #[test]
    fn autoconnect_stops_on_refusal_and_on_quit() {
        for session in [
            Session {
                state: State::Offline {
                    refusal: Some(JoinRejection::ServerFull { max_players: 1 }),
                },
                status: "Server is full".into(),
                ..Default::default()
            },
            Session {
                quit_requested_at: Some(1.0),
                ..Default::default()
            },
        ] {
            let status = session.status.clone();
            let mut world = auto_world(session);
            world.run_system_once(auto_connect).unwrap();
            assert!(!world.resource::<AutoConnect>().armed);
            let session = world.resource::<Session>();
            assert_eq!(session.phase(), ConnectionPhase::Offline);
            assert!(session.live_client().is_none());
            assert_eq!(session.status, status, "the refusal stays visible");
        }
        // Otherwise an idle session does start connecting.
        let mut world = auto_world(Session::default());
        world.run_system_once(auto_connect).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Connecting
        );
    }

    #[test]
    fn connect_never_starts_or_forgets_a_pending_quit() {
        let mut world = World::new();
        world.insert_resource(Session::default());
        world.resource_mut::<Session>().request_quit(3.0);
        world
            .run_system_once(
                move |mut session: ResMut<Session>, mut commands: Commands| {
                    session.connect(ADDRESS, "x", 4.0, &mut commands);
                },
            )
            .unwrap();
        let session = world.resource::<Session>();
        assert_eq!(session.phase(), ConnectionPhase::Offline);
        assert!(session.live_client().is_none());
        assert_eq!(session.quit_requested_at, Some(3.0));
        // A quit requested right after a connect started is not wiped by it.
        let mut session = Session::default();
        let mut world = World::new();
        world
            .run_system_once(move |mut commands: Commands| {
                session.connect(ADDRESS, "x", 1.0, &mut commands);
                session.request_quit(2.0);
                assert_eq!(session.quit_requested_at, Some(2.0));
                assert_eq!(session.phase(), ConnectionPhase::Disconnecting);
            })
            .unwrap();
    }

    #[test]
    fn a_ticket_is_only_held_while_dialing() {
        let mut world = World::new();
        world.insert_resource(Session::default());
        world
            .run_system_once(move |mut s: ResMut<Session>, mut commands: Commands| {
                s.connect_with_ticket(ADDRESS, "x", vec![1, 2, 3], 0.0, &mut commands);
                assert_eq!(s.ticket(), Some(&[1, 2, 3][..]));
                // Sending the hello hands the ticket out exactly once.
                assert_eq!(s.hello_sent(), Some(vec![1, 2, 3]));
                assert!(s.ticket().is_none());
                assert_eq!(s.hello_sent(), None);
            })
            .unwrap();
        // A dial that never reached `Connected` and is torn down drops it too.
        world.insert_resource(Session::default());
        world
            .run_system_once(move |mut s: ResMut<Session>, mut commands: Commands| {
                s.connect_with_ticket(ADDRESS, "x", vec![1], 0.0, &mut commands);
                s.disconnect("timed out");
                assert!(s.ticket().is_none());
                // The next, plain connect must not carry it to another server.
                s.state = State::default();
                s.connect(ADDRESS, "x", 1.0, &mut commands);
                assert!(s.ticket().is_none());
            })
            .unwrap();
    }

    #[test]
    fn a_ticket_is_not_stored_while_a_session_exists() {
        let mut world = World::new();
        world.insert_resource(session_in(State::Handshaking {
            link: link(Entity::PLACEHOLDER),
            since: 0.0,
        }));
        world
            .run_system_once(move |mut s: ResMut<Session>, mut commands: Commands| {
                s.connect_with_ticket(ADDRESS, "x", vec![1], 0.0, &mut commands);
                assert!(s.ticket().is_none());
                assert_eq!(s.phase(), ConnectionPhase::Connecting);
            })
            .unwrap();
    }

    #[test]
    fn no_heartbeat_goes_out_before_the_hello() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.init_resource::<MenuState>();
        // Connected, but with no hello sender yet: the hello cannot go out.
        let entity = world
            .spawn((
                Client::default(),
                RemoteId(PeerId::Server),
                Connected,
                MessageSender::<SessionRequest>::default(),
            ))
            .id();
        world.insert_resource(session_in(State::Dialing {
            link: link(entity),
            since: 0.0,
            ticket: None,
        }));
        world.run_system_once(monitor_connection).unwrap();
        let session = world.resource::<Session>();
        assert!(session.is_dialing());
        assert!(session.state.link().unwrap().last_send.is_none());
        // Once the hello is queued the heartbeat follows it in the same pass.
        world
            .entity_mut(entity)
            .insert(MessageSender::<SessionHello>::default());
        world.run_system_once(monitor_connection).unwrap();
        let session = world.resource::<Session>();
        assert!(!session.is_dialing());
        assert!(session.state.link().unwrap().last_send.is_some());
    }

    #[test]
    fn the_hello_link_exists_from_the_handshake_on() {
        let client = Entity::from_raw_u32(7).unwrap();
        for (state, expected) in [
            (State::default(), None),
            (
                State::Dialing {
                    link: link(client),
                    since: 0.0,
                    ticket: None,
                },
                None,
            ),
            (
                State::Handshaking {
                    link: link(client),
                    since: 0.0,
                },
                Some(client),
            ),
            (playing(client), Some(client)),
            (
                State::Leaving {
                    link: link(client),
                    refusal: None,
                },
                None,
            ),
        ] {
            assert_eq!(session_in(state).hello_link(), expected);
        }
    }

    #[test]
    fn a_hiccup_while_playing_restarts_the_deadline_instead_of_timing_out() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.init_resource::<MenuState>();
        let entity = world
            .spawn((Client::default(), RemoteId(PeerId::Server), Connected))
            .id();
        let mut session = session_in(State::Playing {
            link: link(entity),
            last_reply: 100.0,
        });
        session.admit("x".into());
        world.insert_resource(session);
        world
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs(100));
        // Not synced, no arena: not ready, 100 s after the connect clock began.
        world.run_system_once(monitor_connection).unwrap();
        let session = world.resource::<Session>();
        assert_eq!(session.phase(), ConnectionPhase::Synchronizing);
        assert!(matches!(
            session.state,
            State::Synchronizing { since, .. } if since == 100.0
        ));
        assert!(session.timed_out(100.0).is_none());
        // The usual connect deadline now counts from the hiccup.
        assert!(session.timed_out(104.9).is_none());
        assert!(session.timed_out(110.0).is_some());
    }

    #[test]
    fn local_socket_is_not_enough_to_enter_gameplay() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.init_resource::<MenuState>();
        let entity = world
            .spawn((
                Client::default(),
                RemoteId(PeerId::Server),
                Connected,
                IsSynced::<InputTimeline>::default(),
            ))
            .id();
        world.insert_resource(session_in(State::Handshaking {
            link: link(entity),
            since: 0.0,
        }));
        world.run_system_once(monitor_connection).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Connecting
        );
        assert!(world.resource::<MenuState>().open);
        // The first heartbeat reply moves on to synchronizing.
        world.resource_mut::<Session>().note_reply(0.0);
        world.run_system_once(monitor_connection).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Synchronizing
        );
        // Everything else being ready, an unaccepted join still holds play back.
        world.spawn((shared::Arena, shared::WallSegments::default()));
        world.spawn((
            shared::Player,
            shared::LifeGeneration(1),
            Controlled,
            Predicted,
            InputMarker::<shared::PlayerInput>::default(),
            NativeBuffer::<shared::PlayerInput>::default(),
        ));
        world.run_system_once(monitor_connection).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Synchronizing
        );
        world.resource_mut::<Session>().admit("Final".into());
        world.run_system_once(monitor_connection).unwrap();
        let session = world.resource::<Session>();
        assert_eq!(session.phase(), ConnectionPhase::Playing);
        assert_eq!(session.name(), "Final", "the admitted name wins");
        assert!(!world.resource::<MenuState>().open);
        assert!(world.resource::<MenuState>().block_this_frame);
        // Ordinary frames must not close a menu the player opened manually.
        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(monitor_connection).unwrap();
        assert!(world.resource::<MenuState>().open);
    }

    #[test]
    fn cleanup_waits_for_replicas_before_enabling_reconnect() {
        let mut world = teardown_world(session_in(State::Draining {
            entity: Entity::PLACEHOLDER,
            refusal: None,
        }));
        // No remaining link, but replicated entities may await StateTransition.
        let replica = world.spawn(Replicated).id();
        world.run_system_once(finish_disconnect).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Disconnecting
        );
        world.despawn(replica);
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PendingPress {
                direction: shared::protocol::TurnDirection::Left,
                generation: 0,
            });
        world.resource_mut::<RespawnUi>().pending_generation = Some(3);
        world.run_system_once(finish_disconnect).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Offline
        );
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert!(world.resource::<RespawnUi>().pending_generation.is_none());
    }

    #[test]
    fn send_leave_acts_only_in_leaving_and_always_moves_on() {
        let mut world = World::new();
        let entity = world.spawn(MessageSender::<SessionRequest>::default()).id();
        world.insert_resource(session_in(playing(entity)));
        world.run_system_once(send_leave).unwrap();
        assert_eq!(
            world.resource::<Session>().phase(),
            ConnectionPhase::Playing
        );

        world.resource_mut::<Session>().disconnect("bye");
        assert!(matches!(
            world.resource::<Session>().state,
            State::Leaving { .. }
        ));
        world.run_system_once(send_leave).unwrap();
        assert!(matches!(
            world.resource::<Session>().state,
            State::Unlinking { entity: e, .. } if e == entity
        ));
        // Further runs do nothing.
        world.run_system_once(send_leave).unwrap();
        assert!(matches!(
            world.resource::<Session>().state,
            State::Unlinking { .. }
        ));

        // A lost sender must not leave the teardown hanging in Leaving.
        world.insert_resource(session_in(State::Leaving {
            link: link(Entity::PLACEHOLDER),
            refusal: None,
        }));
        world.run_system_once(send_leave).unwrap();
        assert!(matches!(
            world.resource::<Session>().state,
            State::Unlinking { .. }
        ));
    }

    #[derive(Resource, Default)]
    struct Unlinks(u32);

    #[test]
    fn unlink_acts_only_in_unlinking_and_survives_a_lost_entity() {
        let mut world = World::new();
        world.init_resource::<Unlinks>();
        world.add_observer(|_: On<Disconnect>, mut hits: ResMut<Unlinks>| hits.0 += 1);
        let entity = world.spawn_empty().id();
        for state in [
            playing(entity),
            State::Leaving {
                link: link(entity),
                refusal: None,
            },
        ] {
            world.insert_resource(session_in(state));
            world.run_system_once(unlink_session).unwrap();
            assert_eq!(world.resource::<Unlinks>().0, 0);
        }

        world.insert_resource(session_in(State::Unlinking {
            entity,
            refusal: None,
        }));
        world.run_system_once(unlink_session).unwrap();
        assert_eq!(world.resource::<Unlinks>().0, 1);
        assert!(matches!(
            world.resource::<Session>().state,
            State::Draining { .. }
        ));

        // The entity is gone: no trigger, no panic, still moves on.
        world.despawn(entity);
        world.insert_resource(session_in(State::Unlinking {
            entity,
            refusal: None,
        }));
        world.run_system_once(unlink_session).unwrap();
        assert_eq!(world.resource::<Unlinks>().0, 1);
        assert!(matches!(
            world.resource::<Session>().state,
            State::Draining { .. }
        ));
    }

    #[test]
    fn disconnect_skips_the_leave_only_before_the_hello() {
        let mut session = session_in(State::Dialing {
            link: link(Entity::PLACEHOLDER),
            since: 0.0,
            ticket: None,
        });
        session.disconnect("cancelled");
        assert!(matches!(session.state, State::Unlinking { .. }));

        for state in [
            State::Handshaking {
                link: link(Entity::PLACEHOLDER),
                since: 0.0,
            },
            State::Synchronizing {
                link: link(Entity::PLACEHOLDER),
                since: 0.0,
                last_reply: 0.0,
            },
            playing(Entity::PLACEHOLDER),
        ] {
            let mut session = session_in(state);
            session.disconnect("bye");
            assert!(matches!(session.state, State::Leaving { .. }));
            assert_eq!(session.status, "bye");
        }
        // Disconnecting while offline changes nothing.
        let mut session = session_in(State::default());
        session.disconnect("ignored");
        assert_eq!(session.status, IDLE_STATUS);
    }

    #[test]
    fn a_refusal_survives_the_teardown_until_the_next_connect() {
        let reason = JoinRejection::TicketRequired;
        let mut world = teardown_world(session_in(State::Handshaking {
            link: link(Entity::PLACEHOLDER),
            since: 0.0,
        }));
        world.resource_mut::<Session>().disconnect_refused(reason);
        assert!(world.resource::<Session>().join_rejected());
        world.run_system_once(send_leave).unwrap();
        world.run_system_once(unlink_session).unwrap();
        assert!(world.resource::<Session>().join_rejected());
        world.run_system_once(finish_disconnect).unwrap();
        let session = world.resource::<Session>();
        assert_eq!(session.phase(), ConnectionPhase::Offline);
        assert!(session.join_rejected());
        assert_eq!(session.status, reason.to_string());

        world
            .run_system_once(move |mut s: ResMut<Session>, mut commands: Commands| {
                s.connect(ADDRESS, "x", 1.0, &mut commands);
                assert!(!s.join_rejected());
            })
            .unwrap();
    }
}
