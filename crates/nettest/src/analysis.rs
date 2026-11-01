//! Turning raw traces into per-command facts and summary statistics.

use std::collections::BTreeMap;

use bevy::prelude::Entity;
use shared::PlayerInput;
use shared::protocol::TurnRejectReason;
use tron_zero_server as ts;
use ts::turns::{TurnEvent, TurnEventKind};

use crate::server::TestServer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    Applied {
        tick: u32,
    },
    Rejected(TurnRejectReason),
    /// Accepted but not yet executed or expired.
    Waiting,
}

/// Everything the server logged about one command.
#[derive(Clone, Copy, Debug)]
pub struct Delivery {
    pub seq: u64,
    /// Server tick when the command was received.
    pub received: u32,
    /// Tick the client stamped (unknown for commands rejected at receipt).
    pub intended: Option<u32>,
    pub fate: Fate,
}

impl Delivery {
    /// Server tick at receipt minus the client-stamped tick: negative means
    /// the command arrived that many ticks before its intended tick.
    pub fn arrival_offset(&self) -> Option<i64> {
        self.intended
            .map(|intended| i64::from(self.received) - i64::from(intended))
    }

    /// Execution tick minus intended tick; zero means exactly on time.
    pub fn lateness(&self) -> Option<i64> {
        match (self.intended, self.fate) {
            (Some(intended), Fate::Applied { tick }) => Some(i64::from(tick) - i64::from(intended)),
            _ => None,
        }
    }
}

/// Fold the server's turn log into one `Delivery` per command, in seq order.
pub fn deliveries(server: &TestServer, link: Option<Entity>) -> Vec<Delivery> {
    let mut by_seq: BTreeMap<u64, Delivery> = BTreeMap::new();
    for TurnEvent {
        link: event_link,
        seq,
        server_tick,
        kind,
    } in server.turn_log().0.iter().copied()
    {
        if link.is_some_and(|wanted| wanted != event_link) {
            continue;
        }
        let entry = by_seq.entry(seq).or_insert(Delivery {
            seq,
            received: server_tick,
            intended: None,
            fate: Fate::Waiting,
        });
        match kind {
            TurnEventKind::Queued { intended, .. } => {
                entry.received = server_tick;
                entry.intended = Some(intended);
            }
            TurnEventKind::Applied { .. } => entry.fate = Fate::Applied { tick: server_tick },
            TurnEventKind::Rejected(reason) => entry.fate = Fate::Rejected(reason),
        }
    }
    by_seq.into_values().collect()
}

/// `(tick, input)` of every turn the server simulated for a rider.
pub fn server_turns(server: &TestServer, player_id: &str) -> Vec<(u32, PlayerInput)> {
    server
        .trace()
        .0
        .range((player_id.to_owned(), 0)..=(player_id.to_owned(), u32::MAX))
        .filter(|(_, tick)| tick.executed != PlayerInput::None)
        .map(|((_, tick), state)| (*tick, state.executed))
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Distribution {
    pub n: usize,
    pub min: i64,
    pub p50: i64,
    pub p95: i64,
    pub max: i64,
}

impl Distribution {
    pub fn of(mut values: Vec<i64>) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_unstable();
        let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
        Self {
            n: values.len(),
            min: values[0],
            p50: at(0.5),
            p95: at(0.95),
            max: values[values.len() - 1],
        }
    }
}

impl std::fmt::Display for Distribution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={:3} min={:4} p50={:4} p95={:4} max={:4}",
            self.n, self.min, self.p50, self.p95, self.max
        )
    }
}
