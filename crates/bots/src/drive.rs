//! One tick of decisions for a set of brains: who looks, one snapshot, the
//! level of detail, the inputs. The caller owns the riders and what surrounds
//! a decision (logging, debug overlay, timing): it plugs in through the
//! `decide` closure of [`drive`].

use bevy_ecs::entity::Entity;

use crate::plan::Action;
use crate::schedule::{SchedulerConfig, Tier};
use crate::world::{WorldSnapshot, WorldSnapshotBuilder};
use crate::{Brain, Decision};

/// A bot in the arena, as the caller sees it.
pub struct Seat<'a> {
    pub entity: Entity,
    pub alive: bool,
    pub brain: &'a mut Brain,
}

/// A bot that looks this tick, with what it needs to decide.
pub struct Look<'a> {
    pub entity: Entity,
    pub brain: &'a mut Brain,
    pub world: &'a WorldSnapshot,
    /// The bot's rider in `world.riders`.
    pub index: usize,
    pub tier: Tier,
    pub scheduler: &'a SchedulerConfig,
}

impl Look<'_> {
    /// The brain's decision for this tick.
    pub fn act(&mut self) -> Decision {
        self.brain
            .act(self.world, self.index, self.tier, self.scheduler)
    }
}

/// Run tick `tick` for `seats`; returns one action per seat, in order.
///
/// A living bot looks on its reaction cadence and whenever a planned turn is
/// due (`Brain::wants_view`); the snapshot is built once, in a tick where any
/// bot looks. A bot that does not look holds straight: turns are one-shot, and
/// repeating one would spin the rider into its own trail. `pin` fixes the
/// level of detail; without it, it follows the number of riders alive.
/// `decide` runs once per looking bot and must return its decision, usually
/// `look.act()`.
pub fn drive(
    seats: &mut [Seat<'_>],
    builder: &WorldSnapshotBuilder<'_, '_>,
    tick: u32,
    scheduler: &SchedulerConfig,
    pin: Option<Tier>,
    mut decide: impl FnMut(Look<'_>) -> Decision,
) -> Vec<Action> {
    let mut actions = vec![Action::Hold; seats.len()];
    let looking: Vec<bool> = seats
        .iter()
        .map(|seat| seat.alive && seat.brain.wants_view(tick, scheduler))
        .collect();
    if !looking.contains(&true) {
        return actions;
    }
    let world = builder.build(tick);
    let alive = world.riders.iter().filter(|r| r.alive).count();
    let tier = pin.unwrap_or_else(|| Tier::from_load(alive, scheduler));
    for ((seat, looks), action) in seats.iter_mut().zip(looking).zip(&mut actions) {
        if !looks {
            continue;
        }
        let Some(index) = world.index_of(seat.entity.to_bits()) else {
            continue;
        };
        *action = decide(Look {
            entity: seat.entity,
            brain: &mut *seat.brain,
            world: &world,
            index,
            tier,
            scheduler,
        })
        .action;
    }
    actions
}
