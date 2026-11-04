//! Join admission: the first message of a session decides whether a rider is
//! spawned at all (protocol version, capacity, player name, and on ranked
//! servers a manager-signed join ticket).

use base64::Engine as _;
use bevy::prelude::*;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use lightyear::prelude::*;
use shared::protocol::{
    HelloReply, JoinRejection, MAX_NAME_CHARS, PROTOCOL_ID, SESSION_TIMEOUT_SECS, SessionChannel,
    SessionHello, SessionTicket, name_key, sanitize_name,
};
use std::collections::HashMap;

use crate::ranked::{AccountId, RankedMode, RankedState, TicketClaims, now_ms, verify_ticket};
use crate::session::SessionLease;
use crate::systems::{Human, spawn_human};

/// Human capacity, from `MAX_PLAYERS`. Bots do not count against it: they
/// are a fixed roster, so counting them would make capacity flicker as they
/// die and respawn.
#[derive(Resource, Debug, Clone, Copy)]
pub struct MaxPlayers(pub u32);

impl Default for MaxPlayers {
    fn default() -> Self {
        Self(10)
    }
}

/// Most links that may be waiting for a hello at once. A link only costs a
/// few components until it is admitted (it is not replicated to), so this
/// bounds what stray or spoofed packets can make the server hold; further
/// would-be links are closed on arrival.
pub const MAX_UNADMITTED_LINKS: usize = 32;

/// On a client link until its hello is accepted. Bounds how long a silent
/// (or old-protocol) peer may hold a link without ever getting a rider. A
/// refused link keeps it while it lingers (see `SessionLease`), so it keeps
/// counting against [`MAX_UNADMITTED_LINKS`].
#[derive(Component, Default)]
pub struct AwaitingHello {
    waited_secs: f64,
}

/// A join that passed every check.
#[derive(Debug, PartialEq, Eq)]
struct Admitted {
    name: String,
    /// Set on ranked servers: the verified account.
    account: Option<AccountId>,
}

/// A verified ticket and its base64 form (kept as proof of admission).
type Verified = (TicketClaims, String);

/// Check a join ticket on a ranked server: signature and server binding via
/// [`verify_ticket`], single use via `nonces`, and one live session per account.
fn check_ticket(
    ranked: &RankedState,
    ticket: Option<SessionTicket>,
    now: u64,
    nonces: &mut HashMap<String, u64>,
    connected: &[String],
) -> Result<Verified, JoinRejection> {
    let ticket = ticket.ok_or(JoinRejection::TicketRequired)?;
    // Not registered yet: no key to verify with. The client may simply retry.
    let (server_key_id, keys) = ranked.verification().ok_or(JoinRejection::InvalidTicket)?;
    let claims = verify_ticket(&ticket.ticket, &keys, &server_key_id, now)
        .map_err(|_| JoinRejection::InvalidTicket)?;
    // Forget nonces of tickets that can no longer verify anyway.
    nonces.retain(|_, expires| *expires + 60_000 >= now);
    if nonces.contains_key(&claims.nonce) {
        return Err(JoinRejection::InvalidTicket);
    }
    if connected.contains(&claims.account_id) {
        return Err(JoinRejection::AlreadyConnected);
    }
    nonces.insert(claims.nonce.clone(), claims.expires_at());
    let encoded = base64::engine::general_purpose::STANDARD.encode(&ticket.ticket);
    Ok((claims, encoded))
}

/// Validate a hello against the server's version and capacity, then (lazily,
/// so a refused join does not spend a ticket) its ticket, and pick the final
/// display name (sanitized and made unique among `taken`). On a ranked server
/// the ticket's name wins over what the client typed.
fn admit(
    hello: &SessionHello,
    max: MaxPlayers,
    humans: usize,
    taken: &[String],
    ticket: impl FnOnce() -> Result<Option<Verified>, JoinRejection>,
) -> Result<Admitted, JoinRejection> {
    if hello.protocol != PROTOCOL_ID {
        return Err(JoinRejection::VersionMismatch {
            server: PROTOCOL_ID,
        });
    }
    if humans >= max.0 as usize {
        return Err(JoinRejection::ServerFull { max_players: max.0 });
    }
    let verified = ticket()?;
    let requested = verified
        .as_ref()
        .map_or(hello.name.as_str(), |(claims, _)| {
            claims.display_name.as_str()
        });
    Ok(Admitted {
        name: unique_name(&sanitize_name(requested), taken),
        account: verified.map(|(claims, ticket)| AccountId {
            id: claims.account_id,
            ticket,
        }),
    })
}

/// `base`, or `base 2`, `base 3`... (trimming `base` to stay within the cap)
/// until its [`name_key`] differs from that of every name in `taken`.
fn unique_name(base: &str, taken: &[String]) -> String {
    let taken_keys: Vec<String> = taken.iter().map(|name| name_key(name)).collect();
    let is_taken = |candidate: &str| taken_keys.contains(&name_key(candidate));
    if !is_taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| {
            let suffix = format!(" {n}");
            let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
            let stem: String = base.chars().take(keep).collect();
            format!("{}{suffix}", stem.trim_end())
        })
        .find(|candidate| !is_taken(candidate))
        .expect("an unbounded range always yields a free name")
}

/// Recompute which existing entities an admitted link may see as `Predicted`,
/// `Interpolated` and `Controlled`.
///
/// Lightyear derives those per-link visibility bits in two places: when an
/// entity gets its `PredictionTarget`/`InterpolationTarget`/`ControlledBy`
/// (only for links that already have a `ReplicationSender`), and once when a
/// link's `ClientVisibility` is created (at connection, before any hello).
/// A rider spawned while this link was still waiting for its hello was thus
/// skipped by the first and predates the second, so the link would see that
/// rider with default (everything visible) markers, i.e. as its own. Dropping
/// and re-adding `ClientVisibility` re-runs lightyear's late-joiner pass over
/// every existing rider. Nothing has been sent to the link yet, so no
/// replication state is lost. Call after `ReplicationSender` is inserted and
/// before this link's own rider is spawned (whose hooks then handle it).
fn refresh_link_visibility(commands: &mut Commands, link: Entity) {
    commands
        .entity(link)
        .remove::<ClientVisibility>()
        .insert(ClientVisibility::default());
}

/// Answer each link's first hello: spawn the rider, start replicating to the
/// link and confirm the final name, or reject with a reason and close the link
/// once the refusal has had time to arrive. Links that never say hello are
/// closed after the session timeout.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn handle_hello(
    time: Res<Time<Real>>,
    max: Res<MaxPlayers>,
    mut pending: Query<
        (
            Entity,
            &RemoteId,
            &mut AwaitingHello,
            &mut SessionLease,
            &mut MessageReceiver<SessionHello>,
            &mut MessageReceiver<SessionTicket>,
            &mut MessageSender<HelloReply>,
        ),
        With<Connected>,
    >,
    humans: Query<(), With<Human>>,
    names: Query<&shared::DisplayName>,
    accounts: Query<&AccountId>,
    ranked: Option<Res<RankedMode>>,
    mut nonces: Local<HashMap<String, u64>>,
    mut commands: Commands,
) {
    let mut human_count = humans.iter().count();
    let mut taken: Vec<String> = names.iter().map(|name| name.0.clone()).collect();
    let mut connected: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
    for (link, remote, mut waiting, mut lease, mut receiver, mut tickets, mut sender) in
        &mut pending
    {
        if lease.is_closing() {
            continue;
        }
        waiting.waited_secs += time.delta_secs_f64();
        let Some(hello) = receiver.receive().next() else {
            if waiting.waited_secs >= SESSION_TIMEOUT_SECS {
                warn!(?link, "Client never sent a hello; closing");
                lease.close();
            }
            continue;
        };
        // Sent right before the hello on the same ordered channel, so it is
        // here already; an unranked server never looks at it.
        let ticket = tickets.receive().next();
        let verify = || match &ranked {
            None => Ok(None),
            Some(ranked) => {
                check_ticket(&ranked.0, ticket, now_ms(), &mut nonces, &connected).map(Some)
            }
        };
        match admit(&hello, *max, human_count, &taken, verify) {
            Ok(Admitted { name, account }) => {
                info!(?link, %name, ranked = account.is_some(), "Client admitted");
                // Only an admitted link is replicated to; before this, nothing
                // but the handshake flows to it.
                commands
                    .entity(link)
                    .remove::<AwaitingHello>()
                    .insert(ReplicationSender);
                refresh_link_visibility(&mut commands, link);
                let rider = spawn_human(&mut commands, link, remote.0, name.clone());
                if let Some(account) = account {
                    connected.push(account.id.clone());
                    commands.entity(rider).insert(account);
                }
                human_count += 1;
                taken.push(name.clone());
                sender.send::<SessionChannel>(HelloReply::Accepted { name });
            }
            Err(reason) => {
                warn!(?link, protocol = hello.protocol, %reason, "Client rejected");
                sender.send::<SessionChannel>(HelloReply::Rejected(reason));
                // Keep the link open so a lost rejection is retransmitted.
                lease.close_after_rejection();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anonymous() -> Result<Option<Verified>, JoinRejection> {
        Ok(None)
    }

    /// A ranked state that has registered and learned the test key.
    fn ranked_state() -> RankedState {
        use crate::ranked::testing::{SERVER, keys};
        let state = RankedState::new("k.s".into());
        state.configure(SERVER.into(), keys());
        state
    }

    fn join(account: &str, nonce: &str) -> Option<SessionTicket> {
        use crate::ranked::testing::{SERVER, ticket_for};
        let now = now_ms();
        Some(SessionTicket {
            ticket: ticket_for(account, nonce, SERVER, now, now + 60_000),
        })
    }

    #[test]
    fn ranked_join_needs_a_valid_single_use_ticket_and_one_session_per_account() {
        let state = ranked_state();
        let mut nonces = HashMap::new();
        let check = |ticket, nonces: &mut HashMap<_, _>, connected: &[String]| {
            check_ticket(&state, ticket, now_ms(), nonces, connected).map(|(c, _)| c.account_id)
        };
        assert_eq!(
            check(None, &mut nonces, &[]),
            Err(JoinRejection::TicketRequired)
        );
        assert_eq!(check(join("u1", "n1"), &mut nonces, &[]), Ok("u1".into()));
        // The same ticket again is a replay.
        assert_eq!(
            check(join("u1", "n1"), &mut nonces, &[]),
            Err(JoinRejection::InvalidTicket)
        );
        // A fresh ticket for an account that is already playing.
        assert_eq!(
            check(join("u1", "n2"), &mut nonces, &["u1".into()]),
            Err(JoinRejection::AlreadyConnected)
        );
        // Garbage and tickets for another server are refused.
        let junk = Some(SessionTicket {
            ticket: vec![1; 100],
        });
        assert_eq!(
            check(junk, &mut nonces, &[]),
            Err(JoinRejection::InvalidTicket)
        );
    }

    #[test]
    fn ranked_join_before_registration_is_retryable_not_admitted() {
        let unregistered = RankedState::new("k.s".into());
        let result = check_ticket(
            &unregistered,
            join("u1", "n1"),
            now_ms(),
            &mut HashMap::new(),
            &[],
        );
        assert_eq!(result.map(|_| ()), Err(JoinRejection::InvalidTicket));
    }

    #[test]
    fn the_ticket_name_overrides_the_typed_one_and_a_refused_join_spends_no_ticket() {
        let state = ranked_state();
        let mut nonces = HashMap::new();
        let ticket = join("u1", "n1");
        let full = admit(&hello(PROTOCOL_ID, "Typed"), MaxPlayers(1), 1, &[], || {
            check_ticket(&state, ticket.clone(), now_ms(), &mut nonces, &[]).map(Some)
        });
        assert_eq!(
            full.map(|a| a.name),
            Err(JoinRejection::ServerFull { max_players: 1 })
        );
        assert!(nonces.is_empty(), "full server must not consume the nonce");
        let admitted = admit(&hello(PROTOCOL_ID, "Typed"), MaxPlayers(2), 0, &[], || {
            check_ticket(&state, ticket, now_ms(), &mut nonces, &[]).map(Some)
        })
        .unwrap();
        assert_eq!(admitted.name, "Neo");
        assert_eq!(admitted.account.map(|a| a.id), Some("u1".into()));
    }

    fn hello(protocol: u32, name: &str) -> SessionHello {
        SessionHello {
            protocol,
            name: name.into(),
        }
    }

    #[test]
    fn version_mismatch_is_rejected_before_capacity() {
        let result = admit(
            &hello(PROTOCOL_ID.wrapping_add(1), "a"),
            MaxPlayers(1),
            5,
            &[],
            anonymous,
        );
        assert_eq!(
            result.map(|a| a.name),
            Err(JoinRejection::VersionMismatch {
                server: PROTOCOL_ID
            })
        );
    }

    #[test]
    fn full_server_rejects_and_free_slot_admits() {
        let h = hello(PROTOCOL_ID, "a");
        assert_eq!(
            admit(&h, MaxPlayers(2), 2, &[], anonymous).map(|a| a.name),
            Err(JoinRejection::ServerFull { max_players: 2 })
        );
        assert_eq!(
            admit(&h, MaxPlayers(2), 1, &[], anonymous).map(|a| a.name),
            Ok("a".into())
        );
    }

    #[test]
    fn names_are_sanitized_and_deduplicated_case_insensitively() {
        let taken = vec!["Neo".to_owned(), "neo 2".to_owned()];
        let result = admit(
            &hello(PROTOCOL_ID, " NEO\u{7} "),
            MaxPlayers(9),
            0,
            &taken,
            anonymous,
        );
        assert_eq!(result.map(|a| a.name), Ok("NEO 3".into()));
        assert_eq!(
            admit(&hello(PROTOCOL_ID, ""), MaxPlayers(9), 0, &[], anonymous).map(|a| a.name),
            Ok(shared::protocol::DEFAULT_NAME.into())
        );
    }

    #[test]
    fn lookalike_names_cannot_impersonate_an_existing_player() {
        let taken = vec!["Neo".to_owned()];
        for sneaky in ["N\u{200B}eo", "\u{202E}NEO", "N e o"] {
            let name = admit(
                &hello(PROTOCOL_ID, sneaky),
                MaxPlayers(9),
                0,
                &taken,
                anonymous,
            )
            .unwrap()
            .name;
            assert_ne!(name_key(&name), name_key("Neo"), "{sneaky:?} -> {name:?}");
        }
    }

    #[test]
    fn suffix_never_exceeds_the_length_cap() {
        let long = "x".repeat(MAX_NAME_CHARS);
        let name = unique_name(&long, std::slice::from_ref(&long));
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        assert!(name.ends_with(" 2"));
    }
}
