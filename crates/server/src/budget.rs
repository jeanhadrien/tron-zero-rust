//! Per-link misbehavior budget.
//!
//! A well-behaved client sends a few kinds of message at a low, bounded rate.
//! Anything far above that is a broken or hostile peer, and the cheapest
//! defence is to drop its link: every counter here is a fixed one-second
//! window, so the check costs a few integer operations per message.

use bevy::prelude::*;
use shared::TICK_HZ;

/// What is being counted. Each meter has its own allowance per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meter {
    /// `TurnBatch` messages: one per client tick while turns are unacked, so
    /// 120/s at most; the allowance leaves room for catch-up bursts.
    TurnBatches,
    /// Turn commands refused on receipt (stale life, dead rider, too far
    /// ahead, queue full). Resent copies of decided commands do not count.
    TurnRejections,
    /// Heartbeat and leave messages, which a client sends about once a second.
    SessionRequests,
    /// Respawn requests; a client has at most one in flight.
    RespawnRequests,
    /// `SessionHello` and `SessionTicket`: one of each per join, so a few
    /// retransmissions at most.
    Handshake,
}

impl Meter {
    const COUNT: usize = 5;

    fn allowance_per_sec(self) -> u32 {
        match self {
            Self::TurnBatches => 5 * TICK_HZ as u32,
            Self::TurnRejections => 60,
            Self::SessionRequests | Self::RespawnRequests | Self::Handshake => 10,
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
struct Window {
    started: f64,
    used: u32,
}

/// Lives on the client link entity, so it dies with the connection.
#[derive(Component, Default, Debug)]
pub struct LinkBudget {
    windows: [Window; Meter::COUNT],
}

impl LinkBudget {
    /// Count `n` events of `meter` at time `now` (seconds). Returns `false`
    /// once the current one-second window is over its allowance.
    pub fn spend(&mut self, meter: Meter, n: u32, now: f64) -> bool {
        let window = &mut self.windows[meter as usize];
        if now - window.started >= 1.0 {
            *window = Window {
                started: now,
                used: 0,
            };
        }
        window.used = window.used.saturating_add(n);
        window.used <= meter.allowance_per_sec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowance_resets_every_second_and_meters_are_independent() {
        let mut budget = LinkBudget::default();
        let limit = Meter::SessionRequests.allowance_per_sec();
        for _ in 0..limit {
            assert!(budget.spend(Meter::SessionRequests, 1, 10.0));
        }
        assert!(!budget.spend(Meter::SessionRequests, 1, 10.5));
        assert!(budget.spend(Meter::RespawnRequests, 1, 10.5));
        assert!(budget.spend(Meter::SessionRequests, 1, 11.0));
    }

    #[test]
    fn a_legitimate_turn_stream_stays_far_inside_the_allowance() {
        let mut budget = LinkBudget::default();
        // One batch per tick for ten seconds.
        for tick in 0..10 * TICK_HZ as u32 {
            let now = f64::from(tick) / TICK_HZ;
            assert!(budget.spend(Meter::TurnBatches, 1, now));
        }
    }
}
