//! Shared fixed-tick mechanics with immutable start-of-tick obstacles.

use bevy_ecs::{prelude::*, query::QueryData};
use glam::Vec2;
use lightyear::prelude::{ConfirmedHistory, LocalTimeline, Predicted};

use crate::*;

const EPSILON: f32 = 0.001;

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

#[derive(Clone, Copy, Debug)]
struct Segment {
    a: Vec2,
    b: Vec2,
    owner: Option<Entity>,
    // Distance in segments from the moving head (0 = active).
    from_head: usize,
}

/// Nearest ray/segment contact, including collinear overlap and zero distance.
fn ray_hit(origin: Vec2, direction: Vec2, a: Vec2, b: Vec2) -> Option<f32> {
    let edge = b - a;
    let offset = a - origin;
    let cross = direction.perp_dot(edge);
    if cross.abs() <= EPSILON {
        if offset.perp_dot(direction).abs() > EPSILON {
            return None;
        }
        let t1 = offset.dot(direction);
        let t2 = (b - origin).dot(direction);
        let far = t1.max(t2);
        return (far >= -EPSILON).then_some(t1.min(t2).max(0.0));
    }
    let distance = offset.perp_dot(edge) / cross;
    let along = offset.perp_dot(direction) / cross;
    if distance >= -EPSILON && (-EPSILON..=1.0 + EPSILON).contains(&along) {
        Some(distance.max(0.0))
    } else {
        None
    }
}

fn nearest(
    obstacles: &[Segment],
    entity: Entity,
    origin: Vec2,
    direction: Vec2,
    side: bool,
) -> f32 {
    obstacles
        .iter()
        .filter_map(|segment| {
            let own = segment.owner == Some(entity);
            if own
                && segment.from_head == 0
                && (side || direction.dot(segment.b - segment.a) >= 0.0)
            {
                return None;
            }
            // The newest corner is a departure junction, not a sliding wall.
            if own && side && segment.from_head == 1 {
                return None;
            }
            let hit = ray_hit(origin, direction, segment.a, segment.b)?;
            if own
                && segment.from_head == 1
                && hit <= EPSILON
                && (direction.perp_dot(segment.b - segment.a).abs() > EPSILON
                    || direction.dot(segment.b - segment.a) >= 0.0)
            {
                return None;
            }
            // Boundary walls wind counter-clockwise. At exact contact an
            // inward/tangential heading escapes; outward headings still drain.
            if segment.owner.is_none()
                && !side
                && hit <= EPSILON
                && (segment.b - segment.a).perp_dot(direction) >= 0.0
            {
                return None;
            }
            Some(hit)
        })
        .fold(f32::INFINITY, f32::min)
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
            // During rollback never use an opponent's future rendered state.
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
            owner: Some(player.entity),
            from_head: n - 2 - i,
        }));
    }

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
        match player.input.0.eligible_turn(player.life) {
            PlayerInput::None => {}
            turn => {
                player.trail.turn(player.position.0);
                player.direction.0 = match turn {
                    PlayerInput::TurnLeft => rotate_left(player.direction.0),
                    _ => rotate_right(player.direction.0),
                };
            }
        }
        let origin = player.position.0;
        let heading = player.direction.0;
        let front = nearest(&obstacles, player.entity, origin, heading, false);
        let left = nearest(
            &obstacles,
            player.entity,
            origin,
            rotate_left(heading),
            true,
        );
        let right = nearest(
            &obstacles,
            player.entity,
            origin,
            rotate_right(heading),
            true,
        );
        let normal_step = BASE_SPEED * player.target.0 * TICK_SECS;
        player.colliding.0 = front < SLOW_DOWN_DISTANCE;
        let step = if player.colliding.0 {
            let ratio = (front / SLOW_DOWN_DISTANCE).powi(2);
            player.rubber.0 -= DELTA_STUFF * 0.03 * (1.0 + player.target.0).powi(3);
            (front * ratio).min((front - EPSILON).max(0.0))
        } else {
            player.rubber.0 = (player.rubber.0 + 0.006 * TICK_MS * DELTA_STUFF).min(BASE_RUBBER);
            // Swept sensing: even arbitrarily high slide speeds cannot tunnel.
            normal_step.min((front - EPSILON).max(0.0))
        };
        player.rubber.0 = player.rubber.0.clamp(0.0, BASE_RUBBER);
        if player.rubber.0 == 0.0 {
            disable(&mut player);
            continue;
        }
        player.sliding.0 = left < SLOW_DOWN_DISTANCE || right < SLOW_DOWN_DISTANCE;
        if player.sliding.0 {
            player.target.0 *= 1.003_f32.powf(DELTA_STUFF / 16.666);
        } else if !player.colliding.0 && player.target.0 > 1.0 {
            player.target.0 = (player.target.0 - 0.0003 * DELTA_STUFF).max(1.0);
        }
        // JS inertia lives in TargetSpeedMult: collision changes actual speed,
        // but preserves accumulated boost for escape; boost affects next tick.
        player.speed.0 = step / (BASE_SPEED * TICK_SECS);
        player.velocity.0 = heading * (step * 1000.0);
        player.position.0 += heading * step;
        player.trail.advance(player.position.0, TRAIL_MAX_LENGTH);
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
