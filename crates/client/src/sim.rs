//! Opt-in network impairment for local play-testing against a loopback server.
//!
//! Driven by environment variables so it can never be on by accident:
//!
//! - `TRON_SIM_LATENCY_MS`: target **round-trip** time added to the link.
//! - `TRON_SIM_JITTER_MS`: uniform +/- jitter applied to each direction.
//! - `TRON_SIM_LOSS_PCT`: packet loss percentage applied to each direction.
//!
//! Lightyear's link conditioner only delays what a peer *receives*, so on its
//! own it would shape one direction. To make a client-only simulation
//! symmetric, the downlink uses the conditioner (RTT/2 on the client's receive
//! side) and the uplink uses [`delay_uplink`], which holds outgoing packets
//! for RTT/2 before they reach the socket. Both directions treat every packet
//! independently, so jitter can reorder packets in either one. Loopback adds
//! ~0 ms, so the measured RTT is the requested one (plus up to one frame of
//! quantisation on the uplink).

use std::collections::VecDeque;
use std::time::Duration;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use lightyear::link::SendPayload;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;

use crate::connection::{ConnectionPhase, Session};

/// The active impairment (a resource only when one was requested).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct NetSim {
    pub rtt_ms: u64,
    pub jitter_ms: u64,
    /// Per-direction drop probability in `0.0..=1.0`.
    pub loss: f32,
}

impl NetSim {
    /// Read the `TRON_SIM_*` variables; `None` when nothing is requested.
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(&crate::platform::env_var)
    }

    /// Test seam: read the settings from any key-value lookup.
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let number = |key: &str| -> f64 {
            let Some(raw) = get(key).filter(|v| !v.trim().is_empty()) else {
                return 0.0;
            };
            match raw.trim().parse::<f64>() {
                Ok(value) if value.is_finite() && value >= 0.0 => value,
                _ => {
                    warn!(%key, %raw, "Ignoring invalid network simulation value");
                    0.0
                }
            }
        };
        let sim = Self {
            rtt_ms: number("TRON_SIM_LATENCY_MS").round() as u64,
            jitter_ms: number("TRON_SIM_JITTER_MS").round() as u64,
            loss: (number("TRON_SIM_LOSS_PCT") as f32 / 100.0).min(1.0),
        };
        (sim.rtt_ms > 0 || sim.jitter_ms > 0 || sim.loss > 0.0).then_some(sim)
    }

    /// Short badge text for the HUD, e.g. `sim 20ms` or `sim 80ms ±10 2%`.
    pub fn label(&self) -> String {
        let mut label = format!("sim {}ms", self.rtt_ms);
        if self.jitter_ms > 0 {
            label.push_str(&format!(" ±{}", self.jitter_ms));
        }
        if self.loss > 0.0 {
            label.push_str(&format!(" {}%", (self.loss * 100.0).round()));
        }
        label
    }

    fn one_way(&self) -> Duration {
        Duration::from_millis(self.rtt_ms / 2)
    }

    fn jitter(&self) -> Duration {
        Duration::from_millis(self.jitter_ms)
    }

    /// Install the simulation on every client link this app creates.
    pub fn install(self, app: &mut App) {
        warn!(
            "NETWORK SIMULATION ACTIVE: {} (rtt {} ms split evenly over both directions, \
             jitter ±{} ms and loss {:.1}% per direction)",
            self.label(),
            self.rtt_ms,
            self.jitter_ms,
            self.loss * 100.0
        );
        app.insert_resource(self);
        app.add_observer(
            move |trigger: On<Add, Link>, mut commands: Commands, mut links: Query<&mut Link>| {
                let Ok(mut link) = links.get_mut(trigger.entity) else {
                    return;
                };
                let down = LinkConditionerConfig::new(self.one_way(), self.jitter(), self.loss);
                link.recv.conditioner = Some(lightyear::link::LinkConditioner::new(down));
                commands
                    .entity(trigger.entity)
                    .insert(UplinkDelay::default());
            },
        );
        app.add_systems(
            PostUpdate,
            delay_uplink
                .after(lightyear_transport::plugin::TransportSystems::Send)
                .before(LinkSystems::Send),
        );
    }
}

/// Outgoing packets waiting out their simulated uplink delay.
#[derive(Component, Default)]
pub struct UplinkDelay(VecDeque<(Instant, SendPayload)>);

/// Hold each outgoing packet for RTT/2 +/- jitter (or drop it), releasing the
/// due ones to the socket. Everything is released at once while disconnecting
/// so the leave message is not stranded in the queue.
fn delay_uplink(
    sim: Res<NetSim>,
    session: Res<Session>,
    mut links: Query<(&mut Link, &mut UplinkDelay), With<Client>>,
) {
    let flush = session.phase == ConnectionPhase::Disconnecting;
    let now = Instant::now();
    for (mut link, mut delayed) in &mut links {
        for payload in link.send.drain().collect::<Vec<_>>() {
            if !flush && rand::random::<f32>() < sim.loss {
                continue;
            }
            delayed.0.push_back((now + uplink_delay(&sim), payload));
        }
        for payload in release_due(&mut delayed.0, now, flush) {
            link.send.push(payload);
        }
    }
}

/// Take every packet whose own delay has elapsed (all of them when `flush`),
/// oldest due time first. A held-up packet never blocks later ones, which is
/// what the downlink conditioner does too, so jitter averages out the same way
/// in both directions.
fn release_due<T>(queue: &mut VecDeque<(Instant, T)>, now: Instant, flush: bool) -> Vec<T> {
    let (mut due, waiting): (Vec<_>, Vec<_>) = std::mem::take(queue)
        .into_iter()
        .partition(|(due, _)| flush || *due <= now);
    *queue = waiting.into();
    due.sort_by_key(|(due, _)| *due);
    due.into_iter().map(|(_, payload)| payload).collect()
}

/// One packet's uplink delay: RTT/2 with uniform +/- jitter, never negative.
fn uplink_delay(sim: &NetSim) -> Duration {
    let base = sim.one_way().as_secs_f64();
    let jitter = sim.jitter().as_secs_f64();
    let offset = (rand::random::<f64>() * 2.0 - 1.0) * jitter;
    Duration::from_secs_f64((base + offset).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn nothing_requested_means_no_simulation() {
        assert_eq!(NetSim::from_lookup(&lookup(&[])), None);
        assert_eq!(
            NetSim::from_lookup(&lookup(&[("TRON_SIM_LATENCY_MS", "0")])),
            None
        );
    }

    #[test]
    fn parses_latency_jitter_and_loss() {
        let sim = NetSim::from_lookup(&lookup(&[
            ("TRON_SIM_LATENCY_MS", "80"),
            ("TRON_SIM_JITTER_MS", "10"),
            ("TRON_SIM_LOSS_PCT", "2"),
        ]))
        .unwrap();
        assert_eq!((sim.rtt_ms, sim.jitter_ms), (80, 10));
        assert!((sim.loss - 0.02).abs() < 1e-6);
        assert_eq!(sim.label(), "sim 80ms ±10 2%");
        assert_eq!(sim.one_way(), Duration::from_millis(40));
    }

    #[test]
    fn garbage_values_are_ignored() {
        let sim = NetSim::from_lookup(&lookup(&[
            ("TRON_SIM_LATENCY_MS", "20"),
            ("TRON_SIM_JITTER_MS", "lots"),
            ("TRON_SIM_LOSS_PCT", "-5"),
        ]))
        .unwrap();
        assert_eq!((sim.rtt_ms, sim.jitter_ms, sim.loss), (20, 0, 0.0));
        assert_eq!(sim.label(), "sim 20ms");
    }

    #[test]
    fn a_slow_packet_does_not_hold_back_faster_ones() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut queue = VecDeque::from([(at(50), 'a'), (at(10), 'b'), (at(20), 'c')]);
        assert_eq!(release_due(&mut queue, at(25), false), vec!['b', 'c']);
        assert_eq!(queue.len(), 1);
        assert!(release_due(&mut queue, at(25), false).is_empty());
        assert_eq!(release_due(&mut queue, at(25), true), vec!['a']);
        assert!(queue.is_empty());
    }

    #[test]
    fn uplink_delay_stays_within_jitter_bounds() {
        let sim = NetSim {
            rtt_ms: 40,
            jitter_ms: 5,
            loss: 0.0,
        };
        for _ in 0..200 {
            let delay = uplink_delay(&sim);
            assert!(delay >= Duration::from_millis(15) && delay <= Duration::from_millis(25));
        }
    }
}
