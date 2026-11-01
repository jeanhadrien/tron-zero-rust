//! Application liveness for raw UDP, which has no remote disconnect handshake.

use bevy::prelude::*;
use lightyear::connection::client::Disconnecting;
use lightyear::prelude::*;
use shared::protocol::{
    SESSION_HEARTBEAT_SECS, SESSION_TIMEOUT_SECS, SessionChannel, SessionReply, SessionRequest,
};

/// How long a link whose join was refused stays open after the refusal was
/// sent. The rejection travels on a reliable channel, but only an open link
/// can retransmit it; closing in the same frame would turn one lost packet into
/// a client that never learns why it was refused.
pub const REJECTION_LINGER_SECS: f64 = 1.0;

#[derive(Component, Default)]
pub struct SessionLease {
    nonce: Option<u64>,
    idle_secs: f64,
    leaving: bool,
    /// Seconds left before a refused link is closed regardless.
    linger_secs: Option<f64>,
    /// Seconds until the next heartbeat may be answered.
    reply_cooldown_secs: f64,
}

impl SessionLease {
    /// Mark the session for closing at once (stalled join, abusive client).
    pub fn close(&mut self) {
        self.leaving = true;
    }

    /// Close after a refusal has had a chance to be delivered: when the client
    /// answers with its `Leave`, or after [`REJECTION_LINGER_SECS`].
    pub fn close_after_rejection(&mut self) {
        self.linger_secs.get_or_insert(REJECTION_LINGER_SECS);
    }

    /// Whether the link is being, or is about to be, closed.
    pub fn is_closing(&self) -> bool {
        self.leaving || self.linger_secs.is_some()
    }

    fn receive(&mut self, request: SessionRequest) -> Option<SessionReply> {
        if self.leaving {
            return None;
        }
        if self.linger_secs.is_some() {
            // A refused client that says goodbye has received the refusal.
            if matches!(request, SessionRequest::Leave { .. }) {
                self.leaving = true;
            }
            return None;
        }
        match request {
            SessionRequest::Heartbeat { nonce }
                if self.nonce.is_none_or(|current| current == nonce) =>
            {
                self.nonce = Some(nonce);
                self.idle_secs = 0.0;
                // Replies ride a reliable channel; bounding their rate bounds
                // what a client that never acknowledges them can make us buffer.
                if self.reply_cooldown_secs > 0.0 {
                    return None;
                }
                self.reply_cooldown_secs = SESSION_HEARTBEAT_SECS / 2.0;
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
    let dt = time.delta_secs_f64();
    for (entity, mut lease, mut receiver, mut sender) in &mut clients {
        lease.idle_secs += dt;
        lease.reply_cooldown_secs = (lease.reply_cooldown_secs - dt).max(0.0);
        if let Some(left) = lease.linger_secs.as_mut() {
            *left -= dt;
            if *left <= 0.0 {
                lease.leaving = true;
            }
        }
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
        lease.reply_cooldown_secs = 0.0;
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
    fn heartbeat_replies_are_rate_limited_but_still_keep_the_lease_alive() {
        let mut lease = SessionLease::default();
        assert!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 1 })
                .is_some()
        );
        lease.idle_secs = 2.0;
        for _ in 0..50 {
            assert!(
                lease
                    .receive(SessionRequest::Heartbeat { nonce: 1 })
                    .is_none(),
                "a flood of heartbeats is answered once"
            );
        }
        assert_eq!(lease.idle_secs, 0.0);
    }

    #[test]
    fn refused_link_lingers_until_leave_or_timeout() {
        let mut lease = SessionLease::default();
        lease.close_after_rejection();
        assert!(lease.is_closing());
        assert!(!lease.leaving, "still open to retransmit the refusal");
        // Chatter does not close it or get answered...
        assert!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 1 })
                .is_none()
        );
        assert!(!lease.leaving);
        // ...the client's goodbye does.
        lease.receive(SessionRequest::Leave { nonce: 99 });
        assert!(lease.leaving);
    }

    #[test]
    fn linger_expires_after_the_grace_period() {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        let mut lease = SessionLease::default();
        lease.close_after_rejection();
        let entity = world
            .spawn((
                RemoteId(PeerId::Local(1)),
                Connected,
                lease,
                MessageReceiver::<SessionRequest>::default(),
                MessageSender::<SessionReply>::default(),
            ))
            .id();
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(900));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(!world.get::<SessionLease>(entity).unwrap().leaving);
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(100));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(world.get::<SessionLease>(entity).unwrap().leaving);
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
