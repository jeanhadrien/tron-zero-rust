//! Headless network test harness.
//!
//! A real server app and N headless client apps (the production wiring from
//! `tron-zero-server` / `tron-zero-client`, no window) exchange real UDP over
//! loopback, optionally through Lightyear's receive-side link conditioner for
//! latency, jitter and loss. Scripted key presses go through the real keyboard
//! system; traces on both sides record what each simulation executed per tick.
//!
//! Everything runs on one thread, stepping all apps every ~1 ms of wall time,
//! so a scenario costs its simulated duration in real time. The conditioner
//! uses unseeded randomness: assert invariants, not exact numbers.

pub mod analysis;
pub mod client;
pub mod server;

use std::net::UdpSocket;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use glam::Vec2;
use lightyear::link::LinkConditioner;
use lightyear::prelude::*;

pub use client::{LEFT, RIGHT, TestClient};
pub use server::TestServer;

/// Network tests share the CPU clock; run them one at a time so timing
/// measurements aren't distorted by sibling tests.
pub fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Round-trip impairment, applied symmetrically: each direction gets half the
/// RTT, `jitter_ms` of uniform ± variation and `loss` drop probability.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Impairment {
    pub rtt_ms: u64,
    pub jitter_ms: u64,
    pub loss: f32,
}

impl Impairment {
    pub const NONE: Self = Self::new(0, 0, 0.0);

    pub const fn new(rtt_ms: u64, jitter_ms: u64, loss: f32) -> Self {
        Self {
            rtt_ms,
            jitter_ms,
            loss,
        }
    }

    fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    fn one_way(&self) -> LinkConditionerConfig {
        LinkConditionerConfig::new(
            Duration::from_millis(self.rtt_ms / 2),
            Duration::from_millis(self.jitter_ms),
            self.loss,
        )
    }
}

impl std::fmt::Display for Impairment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rtt {} ms, jitter ±{} ms, loss {:.0}%",
            self.rtt_ms,
            self.jitter_ms,
            self.loss * 100.0
        )
    }
}

/// Condition everything this app receives on any link (client link, or each
/// server-side client link as it appears).
pub(crate) fn install_conditioner(app: &mut App, impairment: Impairment) {
    if impairment.is_none() {
        return;
    }
    let config = impairment.one_way();
    app.add_observer(move |trigger: On<Add, Link>, mut links: Query<&mut Link>| {
        if let Ok(mut link) = links.get_mut(trigger.entity) {
            link.recv.conditioner = Some(LinkConditioner::new(config.clone()));
        }
    });
}

/// A currently free loopback UDP port.
pub fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .and_then(|socket| socket.local_addr())
        .expect("a free loopback port")
        .port()
}

/// One server plus its clients, advanced together.
pub struct Net {
    pub server: TestServer,
    pub clients: Vec<TestClient>,
    port: u16,
    impairment: Impairment,
    last_step: Instant,
}

impl Net {
    /// Server only; add clients with [`Net::add_client`].
    pub fn new(impairment: Impairment) -> Self {
        let port = free_port();
        Self {
            server: TestServer::new(port, impairment),
            clients: Vec::new(),
            port,
            impairment,
            last_step: Instant::now(),
        }
    }

    /// Spawn a client app and start connecting; returns its index.
    pub fn add_client(&mut self) -> usize {
        self.add_client_with("Player", |_| {})
    }

    /// Spawn a client asking for `name`, after `configure` has adjusted it
    /// (e.g. a different protocol version); returns its index.
    pub fn add_client_with(
        &mut self,
        name: &'static str,
        configure: impl FnOnce(&mut TestClient),
    ) -> usize {
        let mut client = TestClient::new(self.impairment);
        configure(&mut client);
        client.connect_as(self.port, name);
        self.clients.push(client);
        self.clients.len() - 1
    }

    /// Spawn a client behind the client-side simulation (`TRON_SIM_*`).
    pub fn add_simulated_client(&mut self, sim: tron_zero_client::sim::NetSim) -> usize {
        let mut client = TestClient::with_sim(self.impairment, sim);
        client.connect(self.port);
        self.clients.push(client);
        self.clients.len() - 1
    }

    /// Advance every app one frame, pacing frames ~1 ms apart.
    pub fn step(&mut self) {
        while self.last_step.elapsed() < Duration::from_millis(1) {
            std::thread::yield_now();
        }
        self.last_step = Instant::now();
        self.server.app.update();
        for client in &mut self.clients {
            client.update();
        }
    }

    /// Step until `done` or `timeout`; returns whether `done` became true.
    pub fn run_until(&mut self, timeout: Duration, mut done: impl FnMut(&mut Net) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.step();
            if done(self) {
                return true;
            }
        }
        false
    }

    /// Step until the server has simulated `ticks` more ticks.
    pub fn run_server_ticks(&mut self, ticks: u32) {
        let target = self.server.tick() + ticks;
        let budget = Duration::from_secs_f64(f64::from(ticks) / shared::TICK_HZ + 10.0);
        assert!(
            self.run_until(budget, |net| net.server.tick() >= target),
            "server did not reach tick {target}"
        );
    }

    /// Connect every client, put each rider on its own lane heading +x from
    /// the arena's lower left, and wait until the initial corrections settle.
    /// Returns each client's player id.
    pub fn join_and_settle(&mut self) -> Vec<String> {
        let ready = self.run_until(Duration::from_secs(15), |net| {
            net.clients
                .iter_mut()
                .all(|c| c.is_playing() && c.player_id().is_some() && c.rider_alive())
        });
        assert!(ready, "clients did not reach Playing in time");
        let ids: Vec<String> = self
            .clients
            .iter_mut()
            .map(|c| c.player_id().expect("player id"))
            .collect();
        for (lane, id) in ids.iter().enumerate() {
            let start = Vec2::new(-1000.0, -1000.0 + 700.0 * lane as f32);
            self.server.place_rider(id, start, Vec2::X);
        }
        // Let replication deliver the pose and any rollback finish.
        self.run_server_ticks(180);
        ids
    }
}

/// Alternating left/right presses `spacing` ticks apart starting at
/// `first_tick` on the client timeline: a staircase that stays inside the
/// arena for dozens of turns when started heading +x from the lower left.
pub fn staircase(client: &mut TestClient, first_tick: u32, turns: usize, spacing: u32) {
    for i in 0..turns {
        let key = if i % 2 == 0 { LEFT } else { RIGHT };
        client.schedule(first_tick + spacing * i as u32, key);
    }
}

impl Net {
    /// UDP port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Delay everything the server receives from clients by `extra` on top of
    /// the configured impairment (queued packets keep their delivery times).
    /// Needs a conditioned network, i.e. an impairment other than `NONE`.
    pub fn set_uplink_latency(&mut self, extra: Duration) {
        let base = self.impairment.one_way();
        let config = LinkConditionerConfig::new(
            base.incoming_latency + extra,
            base.incoming_jitter,
            base.incoming_loss,
        );
        let world = self.server.app.world_mut();
        let mut links = world.query_filtered::<&mut Link, With<LinkOf>>();
        for mut link in links.iter_mut(world) {
            let mut next = LinkConditioner::new(config.clone());
            if let Some(old) = link.recv.conditioner.take() {
                next.time_queue = old.time_queue;
            }
            link.recv.conditioner = Some(next);
        }
    }
}
