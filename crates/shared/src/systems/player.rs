//! Shared fixed-tick mechanics with immutable start-of-tick obstacles.

use bevy_ecs::{prelude::*, query::QueryData};
use glam::Vec2;
use lightyear::prelude::{ConfirmedHistory, LocalTimeline, Predicted};

use crate::physics::{RiderState, RiderStep, Segment, step_riders};
use crate::*;

#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulationRole {
    #[default]
    Server,
    Client,
}

impl SimulationRole {
    fn simulates(self, predicted: bool) -> bool {
        self == Self::Server || predicted
    }
}

#[derive(QueryData)]
#[query_data(mutable)]
pub struct Cycle {
    entity: Entity,
    position: &'static mut Position,
    direction: &'static mut Direction,
    speed: &'static mut SpeedMult,
    target: &'static mut TargetSpeedMult,
    rubber: &'static mut Rubber,
    alive: &'static mut IsAlive,
    sliding: &'static mut IsSliding,
    colliding: &'static mut IsColliding,
    death: &'static mut ShouldHandleDeath,
    trail: &'static mut Trail,
    input: &'static ActionState<PlayerInput>,
    life: Option<&'static LifeGeneration>,
    predicted: Has<Predicted>,
}

#[derive(QueryData)]
pub struct ObstaclePlayer {
    entity: Entity,
    trail: &'static Trail,
    position: &'static Position,
    alive: &'static IsAlive,
    predicted: Has<Predicted>,
    trail_history: Option<&'static ConfirmedHistory<Trail>>,
    position_history: Option<&'static ConfirmedHistory<Position>>,
    alive_history: Option<&'static ConfirmedHistory<IsAlive>>,
}

type ObstacleQuery<'w, 's> = Query<'w, 's, ObstaclePlayer, With<Player>>;
type CycleQuery<'w, 's> = Query<'w, 's, Cycle, With<Player>>;

/// What a rider died against: the owner of the contact it was draining on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathCause {
    Wall,
    OwnTrail,
    /// Another rider's trail; that rider gets the kill.
    Trail(Entity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeathRecord {
    pub tick: Option<u32>,
    pub victim: Entity,
    pub cause: DeathCause,
}

/// Deaths in simulation order, filled by `simulate_players` when present.
/// Only the server inserts it: the system re-runs during client rollback, so
/// a client must never accumulate (or replicate) these. The consumer drains it
/// every tick.
#[derive(Resource, Default, Debug)]
pub struct DeathLog(pub Vec<DeathRecord>);

fn death_cause(victim: Entity, front_owner: Option<u64>) -> DeathCause {
    match front_owner {
        None => DeathCause::Wall,
        Some(bits) if bits == victim.to_bits() => DeathCause::OwnTrail,
        Some(bits) => DeathCause::Trail(Entity::from_bits(bits)),
    }
}

/// Turns, sensing, rubber, speed and trails are one tick transaction.
/// On clients only Predicted players advance; interpolated opponents remain
/// obstacles. Lightyear 0.28 keeps confirmed histories on the same entity,
/// not separate confirmed/predicted copies.
pub fn simulate_players(
    role: Res<SimulationRole>,
    arena: Query<&WallSegments, With<Arena>>,
    timeline: Option<Res<LocalTimeline>>,
    mut players: ParamSet<(ObstacleQuery, CycleQuery)>,
    mut deaths: Option<ResMut<DeathLog>>,
) {
    let tick = timeline.map(|t| t.tick() - 1);
    let mut obstacles = Vec::new();
    for walls in &arena {
        obstacles.extend(walls.0.iter().map(|w| Segment {
            a: Vec2::new(w[0], w[1]),
            b: Vec2::new(w[2], w[3]),
            owner: None,
            from_head: 0,
        }));
    }
    let mut points = Vec::new();
    for player in &players.p0() {
        let (trail, head, alive) = if *role == SimulationRole::Client && !player.predicted {
            // Never use an opponent's rendered (interpolated) state: read its
            // newest confirmed state at or before the tick being simulated.
            // Corners and head of one tick agree: replicon never splits an
            // entity's mutations, and resends every change since the last ack.
            if let Some(tick) = tick {
                let Some(trail) = player.trail_history.and_then(|h| h.get_present(tick)) else {
                    continue;
                };
                let Some(head) = player.position_history.and_then(|h| h.get_present(tick)) else {
                    continue;
                };
                let Some(alive) = player.alive_history.and_then(|h| h.get_present(tick)) else {
                    continue;
                };
                (trail, head, alive)
            } else {
                (player.trail, player.position, player.alive)
            }
        } else {
            (player.trail, player.position, player.alive)
        };
        if !alive.0 {
            continue;
        }
        trail.points_into(head.0, &mut points);
        obstacles.extend(Segment::trail_legs(&points, player.entity.to_bits()));
    }

    // Riders entering the tick alive; trails move into the transaction and
    // come back with its outcome. The move bypasses change detection: only a
    // real change may flag the trail, or it would be replicated every tick.
    let mut entities = Vec::new();
    let mut steps = Vec::new();
    for mut player in &mut players.p1() {
        if !role.simulates(player.predicted) {
            continue;
        }
        if !player.alive.0 {
            if player.death.0 || !player.trail.0.is_empty() {
                disable(&mut player);
            }
            continue;
        }
        if player.rubber.0 <= 0.0 {
            disable(&mut player);
            continue;
        }
        player.death.set_if_neq(ShouldHandleDeath(true));
        entities.push((player.entity, CornersShape::of(&player.trail)));
        steps.push(RiderStep {
            owner: Some(player.entity.to_bits()),
            state: RiderState {
                position: player.position.0,
                direction: player.direction.0,
                target_speed: player.target.0,
                rubber: player.rubber.0,
            },
            turn: player.input.0.eligible_turn(player.life),
            trail: std::mem::take(player.trail.bypass_change_detection()),
        });
    }

    // On clients only the own rider is stepped, so crossings with opponents
    // are not predicted; the server's resolution arrives as a correction.
    let outcomes = step_riders(&mut steps, &obstacles);
    let mut cycles = players.p1();
    for (((entity, before), step), outcome) in entities.into_iter().zip(steps).zip(outcomes) {
        let Ok(mut player) = cycles.get_mut(entity) else {
            continue;
        };
        // Every write is `set_if_neq` (or an explicit change check):
        // replication sends what change detection flags, so a value rewritten
        // unchanged would go out every tick.
        let changed = CornersShape::of(&step.trail) != before;
        *player.trail.bypass_change_detection() = step.trail;
        if changed {
            player.trail.set_changed();
        }
        player.direction.set_if_neq(Direction(outcome.direction));
        if outcome.died {
            if let Some(deaths) = deaths.as_mut() {
                deaths.0.push(DeathRecord {
                    tick: tick.map(|t| t.0),
                    victim: player.entity,
                    cause: death_cause(player.entity, outcome.front_owner),
                });
            }
            disable(&mut player);
            continue;
        }
        player.colliding.set_if_neq(IsColliding(outcome.colliding));
        player.rubber.set_if_neq(Rubber(outcome.rubber));
        player.sliding.set_if_neq(IsSliding(outcome.sliding));
        player
            .target
            .set_if_neq(TargetSpeedMult(outcome.target_speed));
        // JS inertia lives in TargetSpeedMult: collision changes actual speed,
        // but preserves accumulated boost for escape; boost affects next tick.
        player.speed.set_if_neq(SpeedMult(outcome.speed));
        player.position.set_if_neq(Position(outcome.position));
    }
}

/// Enough of a trail to tell whether a tick changed it, in O(1): corners are
/// only pushed at the back (never twice in a row at the same spot) and drained
/// from the front, and one tick turns at most once. So any change moves the
/// length or the last corner.
#[derive(Clone, Copy, PartialEq)]
struct CornersShape(usize, Option<Vec2>);

impl CornersShape {
    fn of(trail: &Trail) -> Self {
        Self(trail.0.len(), trail.0.last().copied())
    }
}

fn disable(player: &mut CycleItem<'_, '_>) {
    player.alive.set_if_neq(IsAlive(false));
    player.death.set_if_neq(ShouldHandleDeath(false));
    player.speed.set_if_neq(SpeedMult(0.0));
    player.target.set_if_neq(TargetSpeedMult(0.0));
    player.rubber.set_if_neq(Rubber(0.0));
    player.sliding.set_if_neq(IsSliding(false));
    player.colliding.set_if_neq(IsColliding(false));
    player.trail.set_if_neq(Trail::default());
}

#[cfg(test)]
#[path = "player_tests.rs"]
mod tests;
