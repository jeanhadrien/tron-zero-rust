//! Answers `shared::probe` discovery datagrams.
//!
//! A raw UDP "connection" is created by any first datagram, so a probe arrives
//! as a brand-new link whose only payload is the request. Before Lightyear's
//! transport sees it, we reply and close the link; anything else is put back
//! untouched and follows the normal join path.

use bevy::prelude::*;
use lightyear::connection::client::Disconnecting;
use lightyear::prelude::*;
use shared::probe::{ProbeReply, decode_request, encode_reply};
use shared::protocol::PROTOCOL_ID;

use crate::admission::{AwaitingHello, MaxPlayers};
use crate::systems::Human;

/// Replies to probes on freshly created client links (a link's first payload). The reply leaves in
/// PostUpdate; the link is despawned in `Last`, so it is flushed first.
#[allow(clippy::type_complexity)]
pub fn answer_probes(
    mut fresh: Query<(Entity, &mut Link), (Added<Link>, With<LinkOf>)>,
    humans: Query<(), With<Human>>,
    max: Res<MaxPlayers>,
    mut commands: Commands,
) {
    for (link_entity, mut link) in &mut fresh {
        let Some(payload) = link.recv.pop() else {
            continue;
        };
        let Some(nonce) = decode_request(payload.as_ref()) else {
            link.recv.push_raw(payload);
            continue;
        };
        let reply = ProbeReply {
            protocol: PROTOCOL_ID,
            players: humans.iter().count().min(u8::MAX as usize) as u8,
            max_players: max.0.min(u8::MAX as u32) as u8,
        };
        link.send(encode_reply(nonce, reply).to_vec().into());
        // A probe is not a join: it must not count against the unadmitted cap.
        commands
            .entity(link_entity)
            .remove::<AwaitingHello>()
            .insert(Disconnecting);
    }
}
