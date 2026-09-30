//! Connection lifecycle independent of menu presentation and server discovery.

use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use lightyear::prelude::client::{Connect, Disconnect, InputDelayConfig, RawClient};
use lightyear::prelude::input::native::{InputMarker, NativeBuffer};
use lightyear::prelude::*;
use shared::protocol::{
    SESSION_HEARTBEAT_SECS, SESSION_TIMEOUT_SECS, SessionChannel, SessionReply, SessionRequest,
};

use crate::input::{InputLifecycle, PendingInput, RespawnUi};
use crate::menu::MenuState;

const CONNECT_TIMEOUT_SECS: f64 = 10.0;

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
            status: "Choose Localhost to join a running server.".into(),
            client: None,
            started_at: 0.0,
            last_reply: None,
            last_send: None,
            unlink_requested: false,
        }
    }
}

impl Session {
    pub fn connect(&mut self, address: SocketAddr, now: f64, commands: &mut Commands) {
        if self.phase != ConnectionPhase::Offline || self.client.is_some() {
            return;
        }
        let client = commands
            .spawn((
                RawClient,
                UdpIo::default(),
                LocalAddr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))),
                PeerAddr(address),
                PredictionManager::default(),
                InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
            ))
            .id();
        *self = Self {
            phase: ConnectionPhase::Connecting,
            status: format!("Connecting to {address}..."),
            client: Some(client),
            started_at: now,
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

    fn timed_out(&self, now: f64) -> Option<&'static str> {
        if self
            .last_reply
            .is_some_and(|last| now - last >= SESSION_TIMEOUT_SECS)
        {
            Some("Server stopped responding. Check the server and retry.")
        } else if self.phase != ConnectionPhase::Playing
            && now - self.started_at >= CONNECT_TIMEOUT_SECS
        {
            Some("Connection/synchronization timed out. Start the localhost server and retry.")
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
    let Ok((connected, synced, disconnected, receiver, sender)) = clients.get_mut(entity) else {
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
        let ready = connected && synced && !arenas.is_empty() && rider_ready;
        if ready {
            if session.phase != ConnectionPhase::Playing {
                info!(
                    ?entity,
                    "Session ready: clock, arena and controlled rider received"
                );
                session.phase = ConnectionPhase::Playing;
                session.status = "Connected to Localhost.".into();
                menu.open = false;
                menu.block_this_frame = true;
            }
        } else {
            session.phase = ConnectionPhase::Synchronizing;
            let status = if !connected {
                "Server answered. Waiting for connection..."
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

/// Wait for Lightyear's state-transition cleanup before allowing another link.
pub fn finish_disconnect(
    mut session: ResMut<Session>,
    clients: Query<Has<Disconnected>, With<Client>>,
    replicated: Query<(), With<Replicated>>,
    mut pending: ResMut<PendingInput>,
    mut lifecycle: ResMut<InputLifecycle>,
    mut respawn: ResMut<RespawnUi>,
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
    session.phase = ConnectionPhase::Offline;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn timeout_distinguishes_no_server_from_lost_session() {
        let mut session = Session {
            phase: ConnectionPhase::Connecting,
            ..Default::default()
        };
        assert!(session.timed_out(9.99).is_none());
        assert!(session.timed_out(10.0).unwrap().contains("synchronization"));
        session.phase = ConnectionPhase::Playing;
        session.last_reply = Some(20.0);
        assert!(session.timed_out(24.99).is_none());
        assert!(
            session
                .timed_out(25.0)
                .unwrap()
                .contains("stopped responding")
        );
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
            .push_back(shared::PlayerInput::TurnLeft);
        world.resource_mut::<RespawnUi>().pending_generation = Some(3);
        world.run_system_once(finish_disconnect).unwrap();
        assert_eq!(world.resource::<Session>().phase, ConnectionPhase::Offline);
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert!(world.resource::<RespawnUi>().pending_generation.is_none());
    }
}
