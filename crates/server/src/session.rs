//! Application liveness for raw UDP, which has no remote disconnect handshake.

use crate::budget::{LinkBudget, Meter};
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

/// Where a session is in its life. `Closing` is absorbing: nothing revives it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Liveness {
    /// Serving. The first heartbeat binds `nonce`; later requests must match it.
    Open { nonce: Option<u64> },
    /// The join was refused: stay open `left` more seconds so the refusal can
    /// be delivered, then close (sooner if the client says goodbye).
    Lingering { left: f64 },
    /// Marked for disconnection (see [`close_sessions`]).
    Closing,
}

#[derive(Component)]
pub struct SessionLease {
    liveness: Liveness,
    idle_secs: f64,
    /// Seconds until the next heartbeat may be answered.
    reply_cooldown_secs: f64,
}

impl Default for SessionLease {
    fn default() -> Self {
        Self {
            liveness: Liveness::Open { nonce: None },
            idle_secs: 0.0,
            reply_cooldown_secs: 0.0,
        }
    }
}

impl SessionLease {
    /// Mark the session for closing at once (stalled join, abusive client).
    pub fn close(&mut self) {
        self.liveness = Liveness::Closing;
    }

    /// Close after a refusal has had a chance to be delivered: when the client
    /// answers with its `Leave`, or after [`REJECTION_LINGER_SECS`].
    pub fn close_after_rejection(&mut self) {
        if matches!(self.liveness, Liveness::Open { .. }) {
            self.liveness = Liveness::Lingering {
                left: REJECTION_LINGER_SECS,
            };
        }
    }

    /// Whether the link is being, or is about to be, closed.
    pub fn is_closing(&self) -> bool {
        !matches!(self.liveness, Liveness::Open { .. })
    }

    /// Whether the link must be disconnected now. Unlike [`is_closing`], a
    /// lingering link is still open.
    ///
    /// [`is_closing`]: SessionLease::is_closing
    fn is_closed(&self) -> bool {
        self.liveness == Liveness::Closing
    }

    /// Whether the first heartbeat has yet to bind the session's nonce.
    fn awaiting_first_heartbeat(&self) -> bool {
        self.liveness == Liveness::Open { nonce: None }
    }

    /// Advance the timers by `dt`: idle and reply clocks run, and a lingering
    /// link closes when its grace is spent.
    fn tick(&mut self, dt: f64) {
        self.idle_secs += dt;
        self.reply_cooldown_secs = (self.reply_cooldown_secs - dt).max(0.0);
        if let Liveness::Lingering { left } = &mut self.liveness {
            *left -= dt;
            if *left <= 0.0 {
                self.liveness = Liveness::Closing;
            }
        }
    }

    /// Close a session (open or lingering) that has been idle for
    /// [`SESSION_TIMEOUT_SECS`]. Returns whether it did.
    fn expire_if_idle(&mut self) -> bool {
        let timed_out = !self.is_closed() && self.idle_secs >= SESSION_TIMEOUT_SECS;
        if timed_out {
            self.liveness = Liveness::Closing;
        }
        timed_out
    }

    fn receive(&mut self, request: SessionRequest) -> Option<SessionReply> {
        let nonce = match &mut self.liveness {
            Liveness::Closing => return None,
            Liveness::Lingering { .. } => {
                // A refused client that says goodbye has received the refusal.
                if matches!(request, SessionRequest::Leave { .. }) {
                    self.liveness = Liveness::Closing;
                }
                return None;
            }
            Liveness::Open { nonce } => nonce,
        };
        match request {
            SessionRequest::Heartbeat { nonce: sent }
                if nonce.is_none_or(|bound| bound == sent) =>
            {
                *nonce = Some(sent);
                self.idle_secs = 0.0;
                // Replies ride a reliable channel; bounding their rate bounds
                // what a client that never acknowledges them can make us buffer.
                if self.reply_cooldown_secs > 0.0 {
                    return None;
                }
                self.reply_cooldown_secs = SESSION_HEARTBEAT_SECS / 2.0;
                Some(SessionReply { nonce: sent })
            }
            SessionRequest::Leave { nonce: sent } if *nonce == Some(sent) => {
                self.liveness = Liveness::Closing;
                None
            }
            _ => {
                warn!("Ignoring session request with a mismatched nonce");
                None
            }
        }
    }
}

pub fn receive_session_requests(
    time: Res<Time<Real>>,
    mut clients: Query<
        (
            Entity,
            &mut SessionLease,
            &mut LinkBudget,
            &mut MessageReceiver<SessionRequest>,
            &mut MessageSender<SessionReply>,
        ),
        With<Connected>,
    >,
) {
    let dt = time.delta_secs_f64();
    let now = time.elapsed_secs_f64();
    for (entity, mut lease, mut budget, mut receiver, mut sender) in &mut clients {
        lease.tick(dt);
        for request in receiver.receive() {
            if !budget.spend(Meter::SessionRequests, 1, now) {
                warn!(?entity, "Session message flood; closing the link");
                lease.close();
                break;
            }
            let first_heartbeat = lease.awaiting_first_heartbeat();
            if let Some(reply) = lease.receive(request) {
                if first_heartbeat {
                    info!(?entity, "Client session heartbeat established");
                }
                sender.send::<SessionChannel>(reply);
            }
        }
        // After the requests: a heartbeat arriving this frame keeps it alive.
        if lease.expire_if_idle() {
            warn!(?entity, "Client heartbeat timed out");
        }
    }
}

pub fn close_sessions(
    clients: Query<(Entity, &SessionLease), With<Connected>>,
    mut commands: Commands,
) {
    for (entity, lease) in &clients {
        if lease.is_closed() {
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
        assert!(!lease.is_closing());
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
        assert!(lease.is_closed());
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
        assert!(
            matches!(lease.liveness, Liveness::Lingering { .. }),
            "still open to retransmit the refusal"
        );
        // Chatter does not close it or get answered...
        assert!(
            lease
                .receive(SessionRequest::Heartbeat { nonce: 1 })
                .is_none()
        );
        assert!(!lease.is_closed());
        // ...the client's goodbye does.
        lease.receive(SessionRequest::Leave { nonce: 99 });
        assert!(lease.is_closed());
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
                LinkBudget::default(),
                MessageReceiver::<SessionRequest>::default(),
                MessageSender::<SessionReply>::default(),
            ))
            .id();
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(900));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(!world.get::<SessionLease>(entity).unwrap().is_closed());
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(100));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(world.get::<SessionLease>(entity).unwrap().is_closed());
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
                LinkBudget::default(),
                MessageReceiver::<SessionRequest>::default(),
                MessageSender::<SessionReply>::default(),
            ))
            .id();
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(4999));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(!world.get::<SessionLease>(entity).unwrap().is_closed());
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(1));
        world.run_system_once(receive_session_requests).unwrap();
        assert!(world.get::<SessionLease>(entity).unwrap().is_closed());
        world.run_system_once(close_sessions).unwrap();
        assert!(world.get::<Disconnecting>(entity).is_some());
        assert!(world.get::<Connected>(entity).is_none());
    }
}
