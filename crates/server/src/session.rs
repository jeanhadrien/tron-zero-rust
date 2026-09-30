//! Application liveness for raw UDP, which has no remote disconnect handshake.

use bevy::prelude::*;
use lightyear::connection::client::Disconnecting;
use lightyear::prelude::*;
use shared::protocol::{SESSION_TIMEOUT_SECS, SessionChannel, SessionReply, SessionRequest};

#[derive(Component, Default)]
pub struct SessionLease {
    nonce: Option<u64>,
    idle_secs: f64,
    leaving: bool,
}

impl SessionLease {
    fn receive(&mut self, request: SessionRequest) -> Option<SessionReply> {
        if self.leaving {
            return None;
        }
        match request {
            SessionRequest::Heartbeat { nonce }
                if self.nonce.is_none_or(|current| current == nonce) =>
            {
                self.nonce = Some(nonce);
                self.idle_secs = 0.0;
                Some(SessionReply { nonce })
            }
            SessionRequest::Leave { nonce } if self.nonce == Some(nonce) => {
                self.leaving = true;
                None
            }
            _ => {
                warn!("Ignoring session request with a mismatched nonce");
                None
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn receive_session_requests(
    time: Res<Time<Real>>,
    mut clients: Query<
        (
            Entity,
            &mut SessionLease,
            &mut MessageReceiver<SessionRequest>,
            &mut MessageSender<SessionReply>,
        ),
        With<Connected>,
    >,
) {
    for (entity, mut lease, mut receiver, mut sender) in &mut clients {
        lease.idle_secs += time.delta_secs_f64();
        for request in receiver.receive() {
            let first_heartbeat = lease.nonce.is_none();
            if let Some(reply) = lease.receive(request) {
                if first_heartbeat {
                    info!(?entity, "Client session heartbeat established");
                }
                sender.send::<SessionChannel>(reply);
            }
        }
        if !lease.leaving && lease.idle_secs >= SESSION_TIMEOUT_SECS {
            warn!(?entity, "Client heartbeat timed out");
            lease.leaving = true;
        }
    }
}

pub fn close_sessions(
    clients: Query<(Entity, &SessionLease), With<Connected>>,
    mut commands: Commands,
) {
    for (entity, lease) in &clients {
        if lease.leaving {
            info!(?entity, "Closing client session");
            // Lightyear performs disconnect observers and link despawn in Last.
            commands.entity(entity).insert(Disconnecting);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    #[test]
    fn lease_accepts_only_its_session_and_cannot_be_revived_after_leave() {
        let mut lease = SessionLease::default();
        assert!(lease.receive(SessionRequest::Leave { nonce: 1 }).is_none());
        assert!(!lease.leaving);
        assert_eq!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 1 })
                .unwrap()
                .nonce,
            1
        );
        lease.idle_secs = 3.0;
        assert!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 2 })
                .is_none()
        );
        assert_eq!(lease.idle_secs, 3.0);
        assert!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 1 })
                .is_some()
        );
        assert_eq!(lease.idle_secs, 0.0);
        lease.receive(SessionRequest::Leave { nonce: 1 });
        assert!(lease.leaving);
        assert!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 1 })
                .is_none()
        );
    }

    #[test]
    fn absent_heartbeat_expires_at_the_five_second_boundary() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        let entity = world
            .spawn((
                RemoteId(PeerId::Local(1)),
                Connected,
                SessionLease::default(),
                MessageReceiver::<SessionRequest>::default(),
                MessageSender::<SessionReply>::default(),
            ))
            .id();
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(4999));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(!world.get::<SessionLease>(entity).unwrap().leaving);
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(1));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(world.get::<SessionLease>(entity).unwrap().leaving);
        world.run_system_once(close_sessions).unwrap();
        assert!(world.get::<Disconnecting>(entity).is_some());
        assert!(world.get::<Connected>(entity).is_none());
    }
}
