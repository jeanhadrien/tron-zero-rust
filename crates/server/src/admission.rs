//! Join admission: the first message of a session decides whether a rider is
//! spawned at all (protocol version, capacity, player name).

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::{
    HelloReply, JoinRejection, MAX_NAME_CHARS, PROTOCOL_VERSION, SESSION_TIMEOUT_SECS,
    SessionChannel, SessionHello, sanitize_name,
};

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

/// On a client link until its hello is accepted. Bounds how long a silent
/// (or old-protocol) peer may hold a link without ever getting a rider.
#[derive(Component, Default)]
pub struct AwaitingHello {
    waited_secs: f64,
}

/// Validate a hello against the server's version and capacity, and pick the
/// final display name (sanitized and made unique among `taken`).
fn admit(
    hello: &SessionHello,
    max: MaxPlayers,
    humans: usize,
    taken: &[String],
) -> Result<String, JoinRejection> {
    if hello.protocol != PROTOCOL_VERSION {
        return Err(JoinRejection::VersionMismatch {
            server: PROTOCOL_VERSION,
        });
    }
    if humans >= max.0 as usize {
        return Err(JoinRejection::ServerFull { max_players: max.0 });
    }
    Ok(unique_name(&sanitize_name(&hello.name), taken))
}

/// `base`, or `base 2`, `base 3`... (trimming `base` to stay within the cap)
/// until it differs, ignoring case, from every name in `taken`.
fn unique_name(base: &str, taken: &[String]) -> String {
    let is_taken = |candidate: &str| {
        taken
            .iter()
            .any(|name| name.to_lowercase() == candidate.to_lowercase())
    };
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

/// Answer each link's first hello: spawn the rider and confirm the final name,
/// or reject with a reason and close the link. Links that never say hello are
/// closed after the session timeout.
#[allow(clippy::type_complexity)]
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
            &mut MessageSender<HelloReply>,
        ),
        With<Connected>,
    >,
    humans: Query<(), With<Human>>,
    names: Query<&shared::DisplayName>,
    mut commands: Commands,
) {
    let mut human_count = humans.iter().count();
    let mut taken: Vec<String> = names.iter().map(|name| name.0.clone()).collect();
    for (link, remote, mut waiting, mut lease, mut receiver, mut sender) in &mut pending {
        waiting.waited_secs += time.delta_secs_f64();
        let Some(hello) = receiver.receive().next() else {
            if waiting.waited_secs >= SESSION_TIMEOUT_SECS {
                warn!(?link, "Client never sent a hello; closing");
                lease.close();
            }
            continue;
        };
        match admit(&hello, *max, human_count, &taken) {
            Ok(name) => {
                info!(?link, %name, "Client admitted");
                spawn_human(&mut commands, link, remote.0, name.clone());
                commands.entity(link).remove::<AwaitingHello>();
                human_count += 1;
                taken.push(name.clone());
                sender.send::<SessionChannel>(HelloReply::Accepted { name });
            }
            Err(reason) => {
                warn!(?link, protocol = hello.protocol, %reason, "Client rejected");
                sender.send::<SessionChannel>(HelloReply::Rejected(reason));
                // close_sessions runs after the transport send, so the
                // rejection is flushed before the link goes away.
                lease.close();
                commands.entity(link).remove::<AwaitingHello>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(protocol: u32, name: &str) -> SessionHello {
        SessionHello {
            protocol,
            name: name.into(),
        }
    }

    #[test]
    fn version_mismatch_is_rejected_before_capacity() {
        let result = admit(&hello(PROTOCOL_VERSION + 1, "a"), MaxPlayers(1), 5, &[]);
        assert_eq!(
            result,
            Err(JoinRejection::VersionMismatch {
                server: PROTOCOL_VERSION
            })
        );
    }

    #[test]
    fn full_server_rejects_and_free_slot_admits() {
        let h = hello(PROTOCOL_VERSION, "a");
        assert_eq!(
            admit(&h, MaxPlayers(2), 2, &[]),
            Err(JoinRejection::ServerFull { max_players: 2 })
        );
        assert_eq!(admit(&h, MaxPlayers(2), 1, &[]), Ok("a".into()));
    }

    #[test]
    fn names_are_sanitized_and_deduplicated_case_insensitively() {
        let taken = vec!["Neo".to_owned(), "neo 2".to_owned()];
        let result = admit(
            &hello(PROTOCOL_VERSION, " NEO\u{7} "),
            MaxPlayers(9),
            0,
            &taken,
        );
        assert_eq!(result, Ok("NEO 3".into()));
        assert_eq!(
            admit(&hello(PROTOCOL_VERSION, ""), MaxPlayers(9), 0, &[]),
            Ok(shared::protocol::DEFAULT_NAME.into())
        );
    }

    #[test]
    fn suffix_never_exceeds_the_length_cap() {
        let long = "x".repeat(MAX_NAME_CHARS);
        let name = unique_name(&long, std::slice::from_ref(&long));
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        assert!(name.ends_with(" 2"));
    }
}
