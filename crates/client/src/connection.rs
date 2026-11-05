//! Connection lifecycle independent of menu presentation and server discovery.

use std::net::SocketAddr;

use bevy::prelude::*;
use lightyear::prelude::client::{Connect, Disconnect, InputDelayConfig, RawClient};
use lightyear::prelude::input::native::{InputMarker, NativeBuffer};
use lightyear::prelude::*;
use shared::protocol::{
    HelloReply, PROTOCOL_ID, SESSION_HEARTBEAT_SECS, SESSION_TIMEOUT_SECS, SessionChannel,
    SessionHello, SessionReply, SessionRequest, SessionTicket, sanitize_name,
};

use crate::input::{InputLifecycle, PendingInput, RespawnUi, TurnAckState, TurnOutbox, TurnSeq};
use crate::menu::MenuState;
use crate::settings::PlayerName;

const CONNECT_TIMEOUT_SECS: f64 = 10.0;
/// Longest a quit waits for the leave to flush before exiting anyway.
const QUIT_GRACE_SECS: f64 = 2.0;
pub const IDLE_STATUS: &str = "Not connected. Browse servers or start a local one.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionPhase {
    #[default]
    Offline,
    Connecting,
    Synchronizing,
    Playing,
    Disconnecting,
}

#[derive(Resource)]
pub struct Session {
    pub phase: ConnectionPhase,
    pub status: String,
    /// Name sent in the hello; replaced by the server's final choice once
    /// admitted.
    pub name: String,
    /// Protocol advertised in the hello (a field so tests can fake a stale build).
    pub protocol: u32,
    /// Manager-signed join ticket for a ranked server, sent once just before
    /// the hello. Set by [`Session::connect_with_ticket`].
    ticket: Option<Vec<u8>>,
    address: Option<SocketAddr>,
    hello_sent: bool,
    admitted: bool,
    /// Set by Quit: exit once the session is torn down (or the grace expires).
    /// Survives `connect`, and once set no new connection may start.
    quit_requested_at: Option<f64>,
    /// The server refused this session's hello. Kept until the next `connect`
    /// so automatic reconnecting can tell a refusal from a dropped link.
    join_rejected: bool,
    client: Option<Entity>,
    started_at: f64,
    last_reply: Option<f64>,
    last_send: Option<f64>,
    unlink_requested: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            phase: ConnectionPhase::Offline,
            status: IDLE_STATUS.into(),
            name: shared::protocol::DEFAULT_NAME.into(),
            protocol: PROTOCOL_ID,
            ticket: None,
            address: None,
            hello_sent: false,
            admitted: false,
            quit_requested_at: None,
            join_rejected: false,
            client: None,
            started_at: 0.0,
            last_reply: None,
            last_send: None,
            unlink_requested: false,
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
        let Endpoint {
            addr: address,
            cert_digest,
        } = endpoint.into();
        self.connect_with(
            address,
            (transport_io(cert_digest), PeerAddr(address)),
            name,
            now,
            commands,
        );
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
        self.ticket = Some(ticket);
        self.connect(endpoint, name, now, commands);
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
        if self.phase != ConnectionPhase::Offline
            || self.client.is_some()
            || self.quit_requested_at.is_some()
        {
            self.ticket = None;
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
        *self = Self {
            phase: ConnectionPhase::Connecting,
            status: format!("Connecting to {address}..."),
            name: name.to_owned(),
            protocol: self.protocol,
            ticket: self.ticket.take(),
            address: Some(address),
            client: Some(client),
            started_at: now,
            quit_requested_at: self.quit_requested_at,
            ..Default::default()
        };
        info!(?client, %address, "Connecting to server");
        // Connect already triggers LinkStart in Lightyear.
        commands.trigger(Connect { entity: client });
    }

    pub fn disconnect(&mut self, status: impl Into<String>) {
        if self.client.is_some() && self.phase != ConnectionPhase::Disconnecting {
            self.phase = ConnectionPhase::Disconnecting;
            self.status = status.into();
            info!(client = ?self.client, reason = %self.status, "Disconnecting session");
        }
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
        self.join_rejected
    }

    fn timed_out(&self, now: f64) -> Option<String> {
        let target = self
            .address
            .map_or_else(|| "the server".to_owned(), |a| a.to_string());
        if self
            .last_reply
            .is_some_and(|last| now - last >= SESSION_TIMEOUT_SECS)
        {
            Some(format!(
                "Lost contact with {target}. Check the server and retry."
            ))
        } else if self.phase != ConnectionPhase::Playing
            && now - self.started_at >= CONNECT_TIMEOUT_SECS
        {
            Some(format!(
                "No response from {target} (timed out). Is the server running and reachable?"
            ))
        } else {
            None
        }
    }
}

#[allow(clippy::type_complexity)]
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
    let Some(entity) = session.client else {
        return;
    };
    if session.phase == ConnectionPhase::Disconnecting {
        return;
    }
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
        && !session.hello_sent
        && let Some(mut hello_sender) = hello_sender
    {
        // Same ordered channel as the heartbeat below, so the server sees the
        // hello first, and the ticket (if any) before the hello.
        if let (Some(ticket), Some(mut ticket_sender)) = (session.ticket.take(), ticket_sender) {
            ticket_sender.send::<SessionChannel>(SessionTicket { ticket });
        }
        hello_sender.send::<SessionChannel>(SessionHello {
            protocol: session.protocol,
            name: session.name.clone(),
        });
        session.hello_sent = true;
    }
    if let Some(mut hello_receiver) = hello_receiver {
        for reply in hello_receiver.receive() {
            match reply {
                HelloReply::Accepted { name } => {
                    info!(%name, "Join accepted");
                    session.name = name;
                    session.admitted = true;
                }
                HelloReply::Rejected(reason) => {
                    warn!(%reason, "Join rejected");
                    session.join_rejected = true;
                    session.disconnect(reason.to_string());
                    menu.open = true;
                    return;
                }
            }
        }
    }
    if let Some(mut receiver) = receiver {
        for reply in receiver.receive() {
            if reply.nonce == entity.to_bits() {
                if session.last_reply.is_none() {
                    info!(?entity, "Server heartbeat confirmed");
                }
                session.last_reply = Some(now);
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
        && session
            .last_send
            .is_none_or(|last| now - last >= SESSION_HEARTBEAT_SECS)
        && let Some(mut sender) = sender
    {
        sender.send::<SessionChannel>(SessionRequest::Heartbeat {
            nonce: entity.to_bits(),
        });
        session.last_send = Some(now);
    }
    if session.last_reply.is_some() {
        // Controlled is replicated; ControlledBy is normally sender-side only.
        let rider_ready = players
            .iter()
            .any(|owner| owner.is_none_or(|c| c.owner == entity));
        let ready = connected && synced && session.admitted && !arenas.is_empty() && rider_ready;
        if ready {
            if session.phase != ConnectionPhase::Playing {
                info!(
                    ?entity,
                    "Session ready: clock, arena and controlled rider received"
                );
                session.phase = ConnectionPhase::Playing;
                session.status = format!("Connected as {}.", session.name);
                menu.open = false;
                menu.block_this_frame = true;
            }
        } else {
            if session.phase == ConnectionPhase::Playing {
                // A momentary hiccup while playing gets a fresh deadline; the
                // connect clock started long ago and would expire at once.
                session.started_at = now;
            }
            session.phase = ConnectionPhase::Synchronizing;
            let status = if !connected {
                "Server answered. Waiting for connection..."
            } else if !session.admitted {
                "Server answered. Waiting for the join to be accepted..."
            } else if !synced {
                "Server answered. Synchronizing clock..."
            } else if arenas.is_empty() {
                "Clock synchronized. Waiting for arena..."
            } else {
                "Clock synchronized. Waiting for controlled rider and input buffer..."
            };
            if session.status != status {
                info!(?entity, %status, "Session readiness pending");
                session.status = status.into();
            }
            menu.open = true;
        }
    }
}

/// Queue leave before the transport sends, then close the socket afterwards.
pub fn send_leave(session: Res<Session>, mut senders: Query<&mut MessageSender<SessionRequest>>) {
    if session.phase == ConnectionPhase::Disconnecting
        && !session.unlink_requested
        && let Some(entity) = session.client
        && let Ok(mut sender) = senders.get_mut(entity)
    {
        sender.send::<SessionChannel>(SessionRequest::Leave {
            nonce: entity.to_bits(),
        });
    }
}

pub fn unlink_session(mut session: ResMut<Session>, mut commands: Commands) {
    if session.phase == ConnectionPhase::Disconnecting && !session.unlink_requested {
        if let Some(entity) = session.client {
            commands.trigger(Disconnect { entity });
        }
        session.unlink_requested = true;
    }
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
    if session.phase == ConnectionPhase::Playing
        || session.join_rejected()
        || session.quit_requested()
    {
        auto.disarm();
    } else if session.phase == ConnectionPhase::Offline && now >= auto.next_attempt {
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
    if session.phase == ConnectionPhase::Offline
        || time.elapsed_secs_f64() - requested_at >= QUIT_GRACE_SECS
    {
        exit.write(AppExit::Success);
    }
}

/// Wait for Lightyear's state-transition cleanup before allowing another link.
#[allow(clippy::too_many_arguments)]
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
    if session.phase != ConnectionPhase::Disconnecting || !session.unlink_requested {
        return;
    }
    let disconnected = session
        .client
        .is_none_or(|entity| clients.get(entity).unwrap_or(true));
    if !disconnected || !replicated.is_empty() {
        return;
    }
    if let Some(entity) = session.client.take()
        && let Ok(mut client) = commands.get_entity(entity)
    {
        client.despawn();
    }
    pending.0.clear();
    *lifecycle = InputLifecycle::default();
    *respawn = RespawnUi::default();
    *turn_seq = TurnSeq::default();
    *turn_acks = TurnAckState::default();
    *outbox = TurnOutbox::default();
    session.phase = ConnectionPhase::Offline;
    session.admitted = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keyboard::PendingPress;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn timeout_distinguishes_no_server_from_lost_session() {
        let mut session = Session {
            phase: ConnectionPhase::Connecting,
            ..Default::default()
        };
        assert!(session.timed_out(9.99).is_none());
        assert!(session.timed_out(10.0).unwrap().contains("timed out"));
        session.phase = ConnectionPhase::Playing;
        session.last_reply = Some(20.0);
        assert!(session.timed_out(24.99).is_none());
        assert!(session.timed_out(25.0).unwrap().contains("Lost contact"));
    }

    fn auto_world(session: Session) -> World {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.insert_resource(PlayerName::default());
        world.insert_resource(session);
        world.insert_resource(AutoConnect {
            endpoint: SocketAddr::from(([127, 0, 0, 1], 9)).into(),
            armed: true,
            next_attempt: 0.0,
        });
        world
    }

    #[test]
    fn autoconnect_stops_on_refusal_and_on_quit() {
        for session in [
            Session {
                join_rejected: true,
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
            assert_eq!(session.phase, ConnectionPhase::Offline);
            assert!(session.client.is_none());
            assert_eq!(session.status, status, "the refusal stays visible");
        }
        // Otherwise an idle session does start connecting.
        let mut world = auto_world(Session::default());
        world.run_system_once(auto_connect).unwrap();
        assert_eq!(
            world.resource::<Session>().phase,
            ConnectionPhase::Connecting
        );
    }

    #[test]
    fn connect_never_starts_or_forgets_a_pending_quit() {
        let mut world = World::new();
        world.insert_resource(Session::default());
        let address = SocketAddr::from(([127, 0, 0, 1], 9));
        world.resource_mut::<Session>().request_quit(3.0);
        world
            .run_system_once(
                move |mut session: ResMut<Session>, mut commands: Commands| {
                    session.connect(address, "x", 4.0, &mut commands);
                },
            )
            .unwrap();
        let session = world.resource::<Session>();
        assert_eq!(session.phase, ConnectionPhase::Offline);
        assert!(session.client.is_none());
        assert_eq!(session.quit_requested_at, Some(3.0));
        // A quit requested right after a connect started is not wiped by it.
        let mut session = Session::default();
        let mut world = World::new();
        world
            .run_system_once(move |mut commands: Commands| {
                session.connect(address, "x", 1.0, &mut commands);
                session.request_quit(2.0);
                assert_eq!(session.quit_requested_at, Some(2.0));
                assert_eq!(session.phase, ConnectionPhase::Disconnecting);
            })
            .unwrap();
    }

    #[test]
    fn a_hiccup_while_playing_restarts_the_deadline_instead_of_timing_out() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.init_resource::<MenuState>();
        let entity = world
            .spawn((Client::default(), RemoteId(PeerId::Server), Connected))
            .id();
        world.insert_resource(Session {
            phase: ConnectionPhase::Playing,
            client: Some(entity),
            started_at: 0.0,
            admitted: true,
            last_reply: Some(100.0),
            ..Default::default()
        });
        world
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs(100));
        // Not synced, no arena: not ready, 100 s after the connect clock began.
        world.run_system_once(monitor_connection).unwrap();
        let session = world.resource::<Session>();
        assert_eq!(session.phase, ConnectionPhase::Synchronizing);
        assert_eq!(session.started_at, 100.0);
        assert!(session.timed_out(100.0).is_none());
        // The usual connect deadline now counts from the hiccup.
        let mut session = Session {
            last_reply: None,
            ..Default::default()
        };
        session.phase = ConnectionPhase::Synchronizing;
        session.started_at = 100.0;
        assert!(session.timed_out(109.9).is_none());
        assert!(session.timed_out(110.0).unwrap().contains("timed out"));
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
        world.insert_resource(Session {
            phase: ConnectionPhase::Connecting,
            client: Some(entity),
            ..Default::default()
        });
        world.run_system_once(monitor_connection).unwrap();
        assert_eq!(
            world.resource::<Session>().phase,
            ConnectionPhase::Connecting
        );
        assert!(world.resource::<MenuState>().open);
        world.resource_mut::<Session>().last_reply = Some(0.0);
        world.run_system_once(monitor_connection).unwrap();
        assert_eq!(
            world.resource::<Session>().phase,
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
            world.resource::<Session>().phase,
            ConnectionPhase::Synchronizing
        );
        world.resource_mut::<Session>().admitted = true;
        world.run_system_once(monitor_connection).unwrap();
        assert_eq!(world.resource::<Session>().phase, ConnectionPhase::Playing);
        assert!(!world.resource::<MenuState>().open);
        assert!(world.resource::<MenuState>().block_this_frame);
        // Ordinary frames must not close a menu the player opened manually.
        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(monitor_connection).unwrap();
        assert!(world.resource::<MenuState>().open);
    }

    #[test]
    fn cleanup_waits_for_replicas_before_enabling_reconnect() {
        let mut world = World::new();
        world.init_resource::<PendingInput>();
        world.init_resource::<InputLifecycle>();
        world.init_resource::<TurnSeq>();
        world.init_resource::<TurnAckState>();
        world.init_resource::<TurnOutbox>();
        world.init_resource::<RespawnUi>();
        // No remaining link, but replicated entities may await StateTransition.
        world.insert_resource(Session {
            phase: ConnectionPhase::Disconnecting,
            unlink_requested: true,
            ..Default::default()
        });
        let replica = world.spawn(Replicated).id();
        world.run_system_once(finish_disconnect).unwrap();
        assert_eq!(
            world.resource::<Session>().phase,
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
        assert_eq!(world.resource::<Session>().phase, ConnectionPhase::Offline);
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert!(world.resource::<RespawnUi>().pending_generation.is_none());
    }
}
