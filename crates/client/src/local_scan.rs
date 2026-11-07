//! Finds game servers running on this machine.
//!
//! While the menu is open and no session exists, a probe datagram goes to a few
//! fixed loopback ports every [`SCAN_PERIOD_SECS`]; servers answer from their
//! link layer (`shared::probe`). A server that stops answering disappears after
//! [`EXPIRY_SECS`]. Native only: a browser cannot send raw UDP, so there the
//! list stays empty (see `browser::local_rows` for the dev-host escape hatch).
//!
//! Each round binds a fresh socket: the server forgets a probe's source address
//! only when it closes the throwaway link, so a reused source port would have
//! every other probe swallowed.

// The probe itself is native-only, so on wasm the bookkeeping is unused.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::net::{Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;

use bevy::prelude::*;
use shared::probe::ProbeReply;

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;

/// The default server port and the next few, for running several servers locally.
pub const LOCAL_PORTS: RangeInclusive<u16> = 5000..=5003;

#[cfg(not(target_family = "wasm"))]
const SCAN_PERIOD_SECS: f64 = 1.5;
/// A little over two periods, so one lost probe does not make an entry flicker.
const EXPIRY_SECS: f64 = 3.5;

/// A local server that answered a probe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalServer {
    pub port: u16,
    pub reply: ProbeReply,
    seen_at: f64,
}

impl LocalServer {
    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }
}

/// Servers currently answering on this machine, lowest port first.
#[derive(Resource, Default)]
pub struct LocalServers {
    found: Vec<LocalServer>,
    #[cfg(not(target_family = "wasm"))]
    round: Option<Round>,
    #[cfg(not(target_family = "wasm"))]
    next_round_at: f64,
    #[cfg(not(target_family = "wasm"))]
    rounds_started: u32,
}

impl LocalServers {
    pub fn found(&self) -> &[LocalServer] {
        &self.found
    }

    /// A list holding `port` as if it had just answered.
    #[cfg(test)]
    pub(crate) fn with_server(port: u16, reply: ProbeReply) -> Self {
        let mut servers = Self::default();
        servers.record(port, reply, 0.0);
        servers
    }

    fn record(&mut self, port: u16, reply: ProbeReply, now: f64) {
        let server = LocalServer {
            port,
            reply,
            seen_at: now,
        };
        match self.found.iter_mut().find(|known| known.port == port) {
            Some(known) => *known = server,
            None => {
                self.found.push(server);
                self.found.sort_by_key(|known| known.port);
            }
        }
    }

    fn expire(&mut self, now: f64) {
        self.found
            .retain(|server| now - server.seen_at <= EXPIRY_SECS);
    }
}

/// Whether the menu is up and idle: the only time scanning is worth its packets.
fn should_scan(menu: &MenuState, session: &Session) -> bool {
    menu.open && session.phase() == ConnectionPhase::Offline
}

#[cfg(not(target_family = "wasm"))]
struct Round {
    socket: std::net::UdpSocket,
    nonce: [u8; 4],
}

#[cfg(not(target_family = "wasm"))]
impl Round {
    /// Bind a fresh loopback socket and probe every local port.
    fn start(nonce: u32) -> std::io::Result<Self> {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        socket.set_nonblocking(true)?;
        let nonce = nonce.to_le_bytes();
        let request = shared::probe::encode_request(nonce);
        for port in LOCAL_PORTS {
            // A closed port is not an error worth reporting: that is the answer.
            let _ = socket.send_to(&request, (Ipv4Addr::LOCALHOST, port));
        }
        Ok(Self { socket, nonce })
    }

    /// Replies that arrived so far, as `(port, reply)`.
    fn replies(&self) -> Vec<(u16, ProbeReply)> {
        use std::io::ErrorKind;
        let mut buf = [0u8; 128];
        let mut out = Vec::new();
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((len, SocketAddr::V4(from)))
                    if from.ip() == &Ipv4Addr::LOCALHOST && LOCAL_PORTS.contains(&from.port()) =>
                {
                    if let Some(reply) = shared::probe::decode_reply(&buf[..len], self.nonce) {
                        out.push((from.port(), reply));
                    }
                }
                Ok(_) => {}
                // Windows reports an earlier ICMP "port unreachable" as a read
                // error on the next receive; it just means a port is closed.
                Err(err) if err.kind() == ErrorKind::ConnectionReset => {}
                Err(_) => break,
            }
        }
        out
    }
}

/// Run the scan: collect answers to the current round, expire silent servers,
/// and start the next round when due. Clears everything when scanning stops, so
/// a stale entry never greets the player coming back to the menu.
#[cfg(not(target_family = "wasm"))]
pub fn scan_local_servers(
    menu: Res<MenuState>,
    session: Res<Session>,
    time: Res<Time<Real>>,
    mut servers: ResMut<LocalServers>,
) {
    if !should_scan(&menu, &session) {
        if servers.round.is_some() || !servers.found.is_empty() {
            servers.round = None;
            servers.found.clear();
        }
        servers.next_round_at = 0.0;
        return;
    }
    let now = time.elapsed_secs_f64();
    if let Some(round) = &servers.round {
        for (port, reply) in round.replies() {
            servers.record(port, reply, now);
        }
    }
    servers.expire(now);
    if now >= servers.next_round_at {
        servers.next_round_at = now + SCAN_PERIOD_SECS;
        servers.rounds_started = servers.rounds_started.wrapping_add(1);
        // The previous round's late replies still count: collect, then replace.
        servers.round = Round::start(servers.rounds_started).ok();
    }
}

/// Browsers cannot probe over UDP; nothing to do.
#[cfg(target_family = "wasm")]
pub fn scan_local_servers() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(players: u8) -> ProbeReply {
        ProbeReply {
            protocol: shared::protocol::PROTOCOL_ID,
            players,
            max_players: 10,
        }
    }

    #[test]
    fn servers_are_sorted_refreshed_and_expire() {
        let mut servers = LocalServers::default();
        servers.record(5002, reply(1), 0.0);
        servers.record(5000, reply(2), 1.0);
        servers.record(5002, reply(3), 2.0);
        let ports: Vec<u16> = servers.found().iter().map(|s| s.port).collect();
        assert_eq!(ports, [5000, 5002]);
        assert_eq!(servers.found()[1].reply.players, 3);

        // 5000 was last seen at 1.0, 5002 at 2.0.
        servers.expire(1.0 + EXPIRY_SECS + 0.1);
        let ports: Vec<u16> = servers.found().iter().map(|s| s.port).collect();
        assert_eq!(ports, [5002]);
        servers.expire(2.0 + EXPIRY_SECS + 0.1);
        assert!(servers.found().is_empty());
    }

    #[test]
    fn scanning_only_happens_in_an_idle_menu() {
        let mut menu = MenuState::default();
        let mut session = Session::default();
        assert!(should_scan(&menu, &session));
        session.force_phase(ConnectionPhase::Playing);
        assert!(!should_scan(&menu, &session));
        session.force_phase(ConnectionPhase::Offline);
        menu.open = false;
        assert!(!should_scan(&menu, &session));
    }
}
