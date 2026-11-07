//! Player-chosen rider look (color + shape).
//!
//! The newest `SetLook` of a link is kept on the link itself, so a look sent
//! before the rider exists (right after the hello) is not lost, and applied to
//! the rider the link owns. Ownership comes from the link, never the message.

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::SetLook;
use shared::{PlayerColor, RiderShape};

use crate::budget::{LinkBudget, Meter};
use crate::session::SessionLease;
use crate::systems::Human;

/// The look a link asked for, used for its rider at admission and kept in sync
/// afterwards. Lives on the link entity.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinkLook {
    pub(crate) color: u32,
    pub(crate) shape: RiderShape,
}

impl Default for LinkLook {
    fn default() -> Self {
        Self {
            color: PlayerColor::DEFAULT.0,
            shape: RiderShape::default(),
        }
    }
}

type OwnedRiders<'w, 's> = Query<
    'w,
    's,
    (
        &'static ControlledBy,
        &'static mut PlayerColor,
        &'static mut RiderShape,
    ),
    With<Human>,
>;

/// Drain this frame's `SetLook` messages. Runs before `admission::handle_hello`
/// so a look that arrives together with the hello is already on the link when
/// the rider is spawned. Links still awaiting their hello are included for the
/// same reason; they have no rider, so the look is only stashed.
///
/// A link sending far more changes than a client ever does (it coalesces to a
/// few per second) is closed, like the other message floods.
#[allow(clippy::type_complexity)]
pub(crate) fn collect_looks(
    time: Res<Time<Real>>,
    mut links: Query<
        (
            Entity,
            &mut MessageReceiver<SetLook>,
            &mut LinkBudget,
            &mut SessionLease,
            &mut LinkLook,
        ),
        With<Connected>,
    >,
    mut riders: OwnedRiders,
) {
    let now = time.elapsed_secs_f64();
    for (link, mut receiver, mut budget, mut lease, mut stash) in &mut links {
        if lease.is_closing() {
            continue;
        }
        let mut newest = None;
        for look in receiver.receive() {
            if !budget.spend(Meter::LookChanges, 1, now) {
                warn!(?link, "Look change flood; closing the link");
                lease.close();
                newest = None;
                break;
            }
            newest = Some(look);
        }
        if let Some(look) = newest {
            apply_look(link, look, &mut stash, &mut riders);
        }
    }
}

/// Keep `look` on the link and copy it to the link's human rider, if any.
fn apply_look(link: Entity, look: SetLook, stash: &mut LinkLook, riders: &mut OwnedRiders) {
    let look = LinkLook {
        color: PlayerColor::masked(look.color).0,
        shape: look.shape,
    };
    *stash = look;
    for (owner, mut color, mut shape) in riders {
        if owner.owner == link {
            color.set_if_neq(PlayerColor(look.color));
            shape.set_if_neq(look.shape);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn rider_for(world: &mut World, owner: Entity) -> Entity {
        world
            .spawn((
                shared::Player,
                Human,
                ControlledBy {
                    owner,
                    lifetime: Lifetime::SessionBased,
                },
            ))
            .id()
    }

    fn send(world: &mut World, link: Entity, look: SetLook) {
        world
            .run_system_once(
                move |mut links: Query<&mut LinkLook>, mut riders: OwnedRiders| {
                    let mut stash = links.get_mut(link).unwrap();
                    apply_look(link, look, &mut stash, &mut riders);
                },
            )
            .unwrap();
    }

    #[test]
    fn a_look_without_a_rider_is_stashed_and_masked() {
        let mut world = World::new();
        let link = world.spawn(LinkLook::default()).id();
        send(
            &mut world,
            link,
            SetLook {
                color: 0xFF12_3456,
                shape: RiderShape::Hexagon,
            },
        );
        let stash = world.get::<LinkLook>(link).unwrap();
        assert_eq!(stash.color, 0x12_3456);
        assert_eq!(stash.shape, RiderShape::Hexagon);
    }

    #[test]
    fn a_look_only_reaches_the_riders_of_its_own_link() {
        let mut world = World::new();
        let mine = world.spawn(LinkLook::default()).id();
        let other = world.spawn(LinkLook::default()).id();
        let my_rider = rider_for(&mut world, mine);
        let other_rider = rider_for(&mut world, other);
        send(
            &mut world,
            mine,
            SetLook {
                color: 0x0A0B0C,
                shape: RiderShape::Chevron,
            },
        );
        assert_eq!(world.get::<PlayerColor>(my_rider).unwrap().0, 0x0A0B0C);
        assert_eq!(
            *world.get::<RiderShape>(my_rider).unwrap(),
            RiderShape::Chevron
        );
        assert_eq!(
            *world.get::<PlayerColor>(other_rider).unwrap(),
            PlayerColor::default()
        );
        assert_eq!(
            *world.get::<RiderShape>(other_rider).unwrap(),
            RiderShape::Circle
        );
    }
}
