//! The player's chosen rider look (color + shape) and its delivery to the server.

use bevy::color::{Hsva, Srgba};
use bevy::prelude::*;
use lightyear::prelude::{Client, MessageSender};
use shared::protocol::{SessionChannel, SetLook};
use shared::{PlayerColor, RiderShape};

use crate::connection::Session;

/// Shortest pause between two `SetLook` sends while the look keeps changing
/// (dragging the color picker); the server budget allows far more.
const LOOK_SEND_MIN_SECS: f64 = 0.2;

/// What this player's rider looks like. Local source of truth: `send_look`
/// forwards it to the server, which replicates it to everyone.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// Packed RGB.
    pub color: u32,
    pub shape: RiderShape,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            color: PlayerColor::DEFAULT.0,
            shape: RiderShape::default(),
        }
    }
}

/// A random vivid look: random hue at full saturation and value, random shape.
pub(crate) fn random_look() -> Look {
    let pick = rand::random_range(0..RiderShape::ALL.len());
    look_from(rand::random::<f32>(), pick)
}

/// Deterministic core of [`random_look`]: `hue` in `[0, 1)`, `shape_index`
/// into [`RiderShape::ALL`] (wrapped).
fn look_from(hue: f32, shape_index: usize) -> Look {
    let rgb = Srgba::from(Hsva::new(hue * 360.0, 1.0, 1.0, 1.0));
    let [r, g, b, _] = rgb.to_u8_array();
    Look {
        color: u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
        shape: RiderShape::ALL[shape_index % RiderShape::ALL.len()],
    }
}

/// What the server was last told, and when.
#[derive(Resource, Default, Debug)]
pub(crate) struct LookSync {
    link: Option<Entity>,
    sent: Option<Look>,
    sent_at: f64,
}

impl LookSync {
    /// Whether `look` must go out now. A link not seen yet gets it at once; on
    /// the same link only a changed value goes out, at most once per
    /// [`LOOK_SEND_MIN_SECS`]. Comparing values (not change flags) means the
    /// final value of a drag is always sent once the pause has elapsed.
    fn due(&self, link: Option<Entity>, look: Look, now: f64) -> bool {
        let Some(link) = link else {
            return false;
        };
        if self.link != Some(link) {
            return true;
        }
        self.sent != Some(look) && now - self.sent_at >= LOOK_SEND_MIN_SECS
    }

    fn record(&mut self, link: Entity, look: Look, now: f64) {
        *self = Self {
            link: Some(link),
            sent: Some(look),
            sent_at: now,
        };
    }
}

/// Sends the look to the server. Must run right after the hello was queued, so
/// `SetLook` follows it on the same ordered channel. The only `SetLook` sender.
pub(crate) fn send_look(
    session: Res<Session>,
    look: Res<Look>,
    time: Res<Time<Real>>,
    mut sync: ResMut<LookSync>,
    mut senders: Query<&mut MessageSender<SetLook>, With<Client>>,
) {
    let link = session.hello_link();
    let now = time.elapsed_secs_f64();
    if !sync.due(link, *look, now) {
        return;
    }
    let Some(link) = link else {
        return;
    };
    let Ok(mut sender) = senders.get_mut(link) else {
        return;
    };
    sender.send::<SessionChannel>(SetLook {
        color: look.color,
        shape: look.shape,
    });
    sync.record(link, *look, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u32) -> Entity {
        Entity::from_raw_u32(n).unwrap()
    }

    fn look(color: u32) -> Look {
        Look {
            color,
            shape: RiderShape::Circle,
        }
    }

    #[test]
    fn nothing_is_sent_without_a_link() {
        assert!(!LookSync::default().due(None, look(1), 100.0));
    }

    #[test]
    fn a_new_link_gets_the_look_at_once() {
        let mut sync = LookSync::default();
        assert!(sync.due(Some(entity(1)), look(1), 0.0));
        sync.record(entity(1), look(1), 0.0);
        assert!(!sync.due(Some(entity(1)), look(1), 50.0), "unchanged");
        assert!(sync.due(Some(entity(2)), look(1), 0.01), "reconnect");
    }

    #[test]
    fn changes_are_throttled_but_the_final_value_still_goes_out() {
        let mut sync = LookSync::default();
        sync.record(entity(1), look(1), 10.0);
        assert!(!sync.due(Some(entity(1)), look(2), 10.15));
        assert!(sync.due(Some(entity(1)), look(2), 10.25));
        sync.record(entity(1), look(2), 10.25);
        // Drag stops on a value; it is sent once the pause is over, not lost.
        assert!(!sync.due(Some(entity(1)), look(3), 10.3));
        assert!(sync.due(Some(entity(1)), look(3), 10.5));
        sync.record(entity(1), look(3), 10.5);
        assert!(!sync.due(Some(entity(1)), look(3), 99.0));
    }

    #[test]
    fn a_shape_change_counts_as_a_change() {
        let mut sync = LookSync::default();
        sync.record(entity(1), look(1), 0.0);
        let square = Look {
            shape: RiderShape::Square,
            ..look(1)
        };
        assert!(sync.due(Some(entity(1)), square, 1.0));
    }

    #[test]
    fn random_looks_are_vivid_and_cover_every_shape() {
        for index in 0..RiderShape::ALL.len() {
            assert_eq!(look_from(0.0, index).shape, RiderShape::ALL[index]);
        }
        assert_eq!(look_from(0.0, 0).color, 0xFF0000);
        let green = look_from(1.0 / 3.0, 0).color;
        assert_eq!(green >> 16 & 0xFF, 0);
        assert_eq!(green >> 8 & 0xFF, 0xFF);
        for _ in 0..50 {
            let color = random_look().color;
            let [r, g, b] = [16, 8, 0].map(|shift| color >> shift & 0xFF);
            assert_eq!(r.max(g).max(b), 0xFF, "full value");
            assert_eq!(r.min(g).min(b), 0, "full saturation");
        }
    }
}
