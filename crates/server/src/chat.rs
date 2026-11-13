//! Player chat: receive lines from admitted clients, label them with the
//! server-known name of the sender and broadcast them to everyone admitted.

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::{ChatChannel, ChatLine, ChatSend, sanitize_chat};
use shared::{DisplayName, PlayerColor};

use crate::admission::AwaitingHello;
use crate::budget::{LinkBudget, Meter};
use crate::session::SessionLease;
use crate::systems::Human;

/// Collect this frame's chat lines from all admitted links and broadcast them.
///
/// The sender is never client-supplied: the name comes from the link's rider.
/// Messages over the soft rate limit, empty after sanitizing, or from a link
/// without a rider yet are dropped silently; a flood far above any human rate
/// closes the link like the other message kinds.
pub fn collect_chat(
    time: Res<Time<Real>>,
    mut links: Query<
        (
            Entity,
            &mut MessageReceiver<ChatSend>,
            &mut LinkBudget,
            &mut SessionLease,
        ),
        (With<Connected>, Without<AwaitingHello>),
    >,
    riders: Query<(&ControlledBy, &DisplayName, &PlayerColor), With<Human>>,
    mut outgoing: Query<&mut MessageSender<ChatLine>, (With<Connected>, Without<AwaitingHello>)>,
) {
    let now = time.elapsed_secs_f64();
    let mut lines: Vec<ChatLine> = Vec::new();
    for (link, mut receiver, mut budget, mut lease) in &mut links {
        for message in receiver.receive() {
            if lease.is_closing() {
                break;
            }
            if !budget.spend(Meter::ChatMessages, 1, now) {
                warn!(?link, "Chat flood; closing the link");
                lease.close();
                break;
            }
            if !budget.allow_chat(now) {
                debug!(?link, "Chat over the rate limit; dropped");
                continue;
            }
            let Some(text) = sanitize_chat(&message.text) else {
                continue;
            };
            let Some((_, name, color)) = riders.iter().find(|(owner, ..)| owner.owner == link)
            else {
                continue;
            };
            lines.push(ChatLine {
                name: name.0.clone(),
                text,
                color: color.0,
            });
        }
    }
    if lines.is_empty() {
        return;
    }
    for mut sender in &mut outgoing {
        for line in &lines {
            sender.send::<ChatChannel>(line.clone());
        }
    }
}
