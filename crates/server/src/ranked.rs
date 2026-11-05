//! Ranked mode: join tickets, roster and result reporting.
//!
//! A server becomes ranked by presenting an operator-issued `RANKED_KEY` when
//! it registers with the manager. From then on every player needs a
//! manager-signed join ticket (verified here, offline), and eliminations are
//! reported back to the manager by the announcer thread. The state shared
//! between Bevy and that thread lives in [`RankedState`].

use bevy::prelude::*;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use shared::Elimination;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::elimination::Eliminations;
use crate::systems::Human;

const SIGNATURE_LEN: usize = 64;
/// Allowed clock difference between manager and this server.
const SKEW_MS: u64 = 30_000;
/// Most events per result batch (the manager accepts 500).
const MAX_BATCH: usize = 500;
/// Eliminations kept while the manager is unreachable; the oldest go first.
const MAX_QUEUE: usize = 4096;

/// Marks a server as ranked. Absent on unranked servers, which admit anyone.
#[derive(Resource, Clone)]
pub struct RankedMode(pub Arc<RankedState>);

/// The verified identity of a human rider (server-only, never replicated).
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct AccountId {
    pub id: String,
    /// The ticket it joined with, base64: proof of admission for the manager.
    pub ticket: String,
}

/// Public key a ticket may be signed with.
#[derive(Clone, Debug)]
pub struct TicketKey {
    pub kid: String,
    pub key: VerifyingKey,
}

impl TicketKey {
    /// From the manager's `{kid, publicKey}` (raw 32-byte key, hex).
    pub fn from_hex(kid: &str, public_key: &str) -> Option<Self> {
        let bytes: [u8; 32] = decode_hex(public_key)?.try_into().ok()?;
        Some(Self {
            kid: kid.to_owned(),
            key: VerifyingKey::from_bytes(&bytes).ok()?,
        })
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// What a valid ticket says. Field names follow `server-manager/src/tickets.ts`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketClaims {
    v: u32,
    kid: String,
    pub account_id: String,
    pub display_name: String,
    server_key_id: String,
    iat: u64,
    exp: u64,
    pub nonce: String,
}

impl TicketClaims {
    /// When the ticket stops being usable (unix ms), for nonce bookkeeping.
    pub fn expires_at(&self) -> u64 {
        self.exp
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketError {
    Malformed,
    UnknownKey,
    BadSignature,
    WrongServer,
    Expired,
}

/// Check a ticket: `payload JSON || 64-byte Ed25519 signature`, signed over the
/// exact payload bytes. `server_key_id` is this server's own ranked-key id, so
/// a ticket issued for another server is refused.
pub fn verify_ticket(
    ticket: &[u8],
    keys: &[TicketKey],
    server_key_id: &str,
    now_ms: u64,
) -> Result<TicketClaims, TicketError> {
    if ticket.len() <= SIGNATURE_LEN {
        return Err(TicketError::Malformed);
    }
    let (body, signature) = ticket.split_at(ticket.len() - SIGNATURE_LEN);
    // The kid is read before the signature is checked only to pick the key;
    // nothing else in the claims is trusted until the signature holds.
    let claims: TicketClaims = serde_json::from_slice(body).map_err(|_| TicketError::Malformed)?;
    let key = keys
        .iter()
        .find(|k| k.kid == claims.kid)
        .ok_or(TicketError::UnknownKey)?;
    let signature = Signature::from_slice(signature).map_err(|_| TicketError::Malformed)?;
    key.key
        .verify(body, &signature)
        .map_err(|_| TicketError::BadSignature)?;
    if claims.v != 1 || claims.server_key_id != server_key_id {
        return Err(TicketError::WrongServer);
    }
    if now_ms + SKEW_MS < claims.iat || now_ms > claims.exp + SKEW_MS {
        return Err(TicketError::Expired);
    }
    Ok(claims)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A player to list in the heartbeat roster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterEntry {
    pub account_id: String,
    pub ticket: String,
}

/// One result batch as the manager expects it.
#[derive(Clone, Debug, Serialize)]
pub struct Batch {
    pub session: String,
    pub seq: u64,
    pub events: Vec<Elimination<String>>,
}

#[derive(Default)]
struct Inner {
    key_id: Option<String>,
    keys: Vec<TicketKey>,
    roster: Vec<RosterEntry>,
    session: String,
    next_seq: u64,
    queue: VecDeque<Elimination<String>>,
    /// Sent but not yet acknowledged; resent unchanged until it is.
    inflight: Option<Batch>,
}

/// Ranked state shared by Bevy (admission, roster, eliminations) and the
/// announcer thread (registration keys, heartbeats, result delivery).
pub struct RankedState {
    inner: Mutex<Inner>,
}

impl RankedState {
    pub fn new() -> Self {
        let inner = Inner {
            // New per process: the manager resets its `seq` cursor with it.
            session: format!("{:016x}", rand::random::<u64>()),
            next_seq: 1,
            ..Inner::default()
        };
        Self {
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for RankedState {
    fn default() -> Self {
        Self::new()
    }
}

impl RankedState {
    /// Store what the manager's register response taught us.
    pub fn configure(&self, key_id: String, keys: Vec<TicketKey>) {
        let mut inner = self.lock();
        inner.key_id = Some(key_id);
        inner.keys = keys;
    }

    /// This server's ranked-key id and the ticket keys, once registered.
    pub fn verification(&self) -> Option<(String, Vec<TicketKey>)> {
        let inner = self.lock();
        Some((inner.key_id.clone()?, inner.keys.clone()))
    }

    pub fn set_roster(&self, roster: Vec<RosterEntry>) {
        self.lock().roster = roster;
    }

    pub fn roster(&self) -> Vec<RosterEntry> {
        self.lock().roster.clone()
    }

    pub fn push_eliminations(&self, events: impl IntoIterator<Item = Elimination<String>>) {
        let mut inner = self.lock();
        inner.queue.extend(events);
        while inner.queue.len() > MAX_QUEUE {
            inner.queue.pop_front();
        }
    }

    /// The batch to deliver now: the unacknowledged one, else a fresh one.
    pub fn next_batch(&self) -> Option<Batch> {
        let mut inner = self.lock();
        if inner.inflight.is_none() && !inner.queue.is_empty() {
            let take = inner.queue.len().min(MAX_BATCH);
            let events: Vec<_> = inner.queue.drain(..take).collect();
            let batch = Batch {
                session: inner.session.clone(),
                seq: inner.next_seq,
                events,
            };
            inner.next_seq += 1;
            inner.inflight = Some(batch);
        }
        inner.inflight.clone()
    }

    /// The manager has the batch (applied, or it was already): forget it.
    pub fn acknowledge(&self, seq: u64) {
        let mut inner = self.lock();
        if inner.inflight.as_ref().is_some_and(|b| b.seq == seq) {
            inner.inflight = None;
        }
    }
}

/// Copy the connected, ticketed humans into the heartbeat roster.
pub fn publish_roster(ranked: Res<RankedMode>, humans: Query<&AccountId, With<Human>>) {
    ranked.0.set_roster(
        humans
            .iter()
            .map(|account| RosterEntry {
                account_id: account.id.clone(),
                ticket: account.ticket.clone(),
            })
            .collect(),
    );
}

/// Hand resolved eliminations to the announcer thread for delivery.
pub fn forward_eliminations(ranked: Res<RankedMode>, mut resolved: ResMut<Eliminations>) {
    if !resolved.0.is_empty() {
        ranked.0.push_eliminations(resolved.0.drain(..));
    }
}

/// Ticket fixtures: a fixed signing key and tickets shaped like the manager's.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub const SERVER: &str = "k1";

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    pub fn keys() -> Vec<TicketKey> {
        vec![TicketKey {
            kid: "abcd".into(),
            key: signing_key().verifying_key(),
        }]
    }

    /// A ticket for `account` with a fresh `nonce`, valid over `[iat, exp]`.
    pub fn ticket_for(account: &str, nonce: &str, server: &str, iat: u64, exp: u64) -> Vec<u8> {
        let body = format!(
            r#"{{"v":1,"kid":"abcd","accountId":"{account}","displayName":"Neo","serverKeyId":"{server}","roomId":"r","iat":{iat},"exp":{exp},"nonce":"{nonce}"}}"#
        );
        let mut bytes = body.clone().into_bytes();
        bytes.extend_from_slice(&signing_key().sign(body.as_bytes()).to_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn ticket(server: &str, iat: u64, exp: u64) -> Vec<u8> {
        ticket_for("u1", "n1", server, iat, exp)
    }

    #[test]
    fn a_fresh_ticket_for_this_server_verifies() {
        let claims = verify_ticket(&ticket(SERVER, 1_000, 61_000), &keys(), SERVER, 2_000).unwrap();
        assert_eq!(claims.account_id, "u1");
        assert_eq!(claims.display_name, "Neo");
    }

    #[test]
    fn tampering_wrong_server_expiry_and_unknown_keys_are_refused() {
        let good = ticket(SERVER, 1_000, 61_000);
        let mut tampered = good.clone();
        tampered[20] ^= 1;
        assert!(verify_ticket(&tampered, &keys(), SERVER, 2_000).is_err());
        assert_eq!(
            verify_ticket(&ticket("k2", 1_000, 61_000), &keys(), SERVER, 2_000).unwrap_err(),
            TicketError::WrongServer
        );
        assert_eq!(
            verify_ticket(&good, &keys(), SERVER, 61_000 + SKEW_MS + 1).unwrap_err(),
            TicketError::Expired
        );
        assert_eq!(
            verify_ticket(&good, &[], SERVER, 2_000).unwrap_err(),
            TicketError::UnknownKey
        );
        assert_eq!(
            verify_ticket(&good[..40], &keys(), SERVER, 2_000).unwrap_err(),
            TicketError::Malformed
        );
    }

    #[test]
    fn batches_are_resent_until_acknowledged() {
        let state = RankedState::new();
        assert!(state.next_batch().is_none());
        state.push_eliminations([Elimination::Mutual {
            tick: None,
            a: "x".into(),
            b: "y".into(),
        }]);
        let first = state.next_batch().unwrap();
        assert_eq!(state.next_batch().unwrap().seq, first.seq);
        state.acknowledge(first.seq);
        assert!(state.next_batch().is_none());
    }
}
