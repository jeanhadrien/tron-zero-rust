//! Shared fixed-tick mechanics with immutable start-of-tick obstacles.

use bevy_ecs::{prelude::*, query::QueryData};
use glam::Vec2;
use lightyear::prelude::{ConfirmedHistory, LocalTimeline, Predicted};

use crate::physics::{Move, RiderState, Segment, crash, resolve_crossings, step_rider};
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
    velocity: &'static mut Velocity,
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
    alive: &'static IsAlive,
    predicted: Has<Predicted>,
    trail_history: Option<&'static ConfirmedHistory<Trail>>,
    alive_history: Option<&'static ConfirmedHistory<IsAlive>>,
}

type ObstacleQuery<'w, 's> = Query<'w, 's, ObstaclePlayer, With<Player>>;
type CycleQuery<'w, 's> = Query<'w, 's, Cycle, With<Player>>;

/// Turns, sensing, rubber, speed and trails are one tick transaction.
/// On clients only Predicted players advance; interpolated opponents remain
/// obstacles. Lightyear 0.28 keeps confirmed histories on the same entity,
/// not separate confirmed/predicted copies.
pub fn simulate_players(
    role: Res<SimulationRole>,
    arena: Query<&WallSegments, With<Arena>>,
    timeline: Option<Res<LocalTimeline>>,
    mut players: ParamSet<(ObstacleQuery, CycleQuery)>,
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
    for player in &players.p0() {
        let (trail, alive) = if *role == SimulationRole::Client && !player.predicted {
            // Never use an opponent's rendered (interpolated) state: read its
            // newest confirmed state at or before the tick being simulated.
            if let Some(tick) = tick {
                let Some(trail) = player.trail_history.and_then(|h| h.get_present(tick)) else {
                    continue;
                };
                let Some(alive) = player.alive_history.and_then(|h| h.get_present(tick)) else {
                    continue;
                };
                (trail, alive)
            } else {
                (player.trail, player.alive)
            }
        } else {
            (player.trail, player.alive)
        };
        if !alive.0 {
            continue;
        }
        let n = trail.0.len();
        obstacles.extend(trail.0.windows(2).enumerate().map(|(i, p)| Segment {
            a: p[0],
            b: p[1],
            owner: Some(player.entity.to_bits()),
            from_head: n - 2 - i,
        }));
    }

    let mut moved = Vec::new();
    let mut moves = Vec::new();
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
        player.death.0 = true;
        if player.trail.0.is_empty() {
            *player.trail = Trail::new(player.position.0);
        }
        let (cruise, rubber) = (player.target.0, player.rubber.0);
        let outcome = step_rider(
            Some(player.entity.to_bits()),
            RiderState {
                position: player.position.0,
                direction: player.direction.0,
                target_speed: player.target.0,
                rubber: player.rubber.0,
            },
            player.input.0.eligible_turn(player.life),
            &obstacles,
            &mut player.trail,
        );
        player.direction.0 = outcome.direction;
        if outcome.died {
            disable(&mut player);
            continue;
        }
        player.colliding.0 = outcome.colliding;
        player.rubber.0 = outcome.rubber;
        player.sliding.0 = outcome.sliding;
        player.target.0 = outcome.target_speed;
        // JS inertia lives in TargetSpeedMult: collision changes actual speed,
        // but preserves accumulated boost for escape; boost affects next tick.
        player.speed.0 = outcome.speed;
        player.velocity.0 = outcome.direction * (outcome.step * 1000.0);
        moved.push((player.entity, cruise, rubber));
        moves.push(Move {
            from: player.position.0,
            direction: outcome.direction,
            length: outcome.step,
        });
        player.position.0 = outcome.position;
    }

    // Second pass: trails grown this tick, which the snapshot could not see.
    // On clients only the own rider moves here, so this is server-side in
    // practice; a client that crosses an opponent mid-tick is corrected.
    let mut cycles = players.p1();
    // Being stopped is a collision: it costs rubber like facing a wall, or two
    // riders tied at a crossing would wait there forever, regenerating.
    let stops = resolve_crossings(&moves);
    for ((&(entity, cruise, rubber), m), length) in moved.iter().zip(&moves).zip(stops) {
        let Some(length) = length else {
            continue;
        };
        let Ok(mut player) = cycles.get_mut(entity) else {
            continue;
        };
        let (target, rubber) = crash(cruise, rubber);
        if rubber == 0.0 {
            disable(&mut player);
            continue;
        }
        player.target.0 = target;
        player.rubber.0 = rubber;
        player.sliding.0 = false;
        let position = m.from + m.direction * length;
        player.position.0 = position;
        player.velocity.0 = m.direction * (length * 1000.0);
        player.speed.0 = length / (BASE_SPEED * TICK_SECS);
        player.colliding.0 = true;
        player.trail.advance(position, TRAIL_MAX_LENGTH);
    }
}

fn disable(player: &mut CycleItem<'_, '_>) {
    player.alive.0 = false;
    player.death.0 = false;
    player.speed.0 = 0.0;
    player.target.0 = 0.0;
    player.velocity.0 = Vec2::ZERO;
    player.rubber.0 = 0.0;
    player.sliding.0 = false;
    player.colliding.0 = false;
    player.trail.0.clear();
}

#[cfg(test)]
#[path = "player_tests.rs"]
mod tests;
