//! Pure lightcycle physics shared by the authoritative simulation and
//! server-side forecasting, so prediction cannot drift from the sim.
//!
//! `step_rider` is a transaction over plain data: it never touches ECS. The
//! sim reads the resulting state back into components; the forecaster applies
//! it to a `WorldSnapshot`.

use glam::Vec2;

use crate::*;

const EPSILON: f32 = 0.001;

/// A ray blocker: an arena wall or a rider trail segment.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub a: Vec2,
    pub b: Vec2,
    /// Owning rider's entity bits, or `None` for arena walls.
    pub owner: Option<u64>,
    /// Distance in segments from the moving head (0 = active).
    pub from_head: usize,
}

/// Nearest ray/segment contact, including collinear overlap and zero distance.
pub fn ray_hit(origin: Vec2, direction: Vec2, a: Vec2, b: Vec2) -> Option<f32> {
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

/// Every obstacle the ray touches (with its distance), honouring the
/// self-trail exclusions the sim uses at turn junctions so a rider is never
/// blocked by its own active edge.
fn contacts(
    obstacles: &[Segment],
    owner: Option<u64>,
    origin: Vec2,
    direction: Vec2,
    side: bool,
) -> impl Iterator<Item = (f32, &Segment)> {
    obstacles.iter().filter_map(move |segment| {
        let own = owner.is_some() && segment.owner == owner;
        if own && segment.from_head == 0 && (side || direction.dot(segment.b - segment.a) >= 0.0) {
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
        Some((hit, segment))
    })
}

/// Distance to the nearest obstacle along a ray.
pub fn nearest(
    obstacles: &[Segment],
    owner: Option<u64>,
    origin: Vec2,
    direction: Vec2,
    side: bool,
) -> f32 {
    contacts(obstacles, owner, origin, direction, side)
        .map(|(hit, _)| hit)
        .fold(f32::INFINITY, f32::min)
}

/// Grind strength in `0..=1` for a side ray: 1 touching an obstacle, falling
/// linearly to 0 at `SLIDE_DISTANCE`. Arena walls count as farther away
/// (`ARENA_WALL_GRIND`) so the boundary is a weaker accelerator than trails.
fn grind_proximity(
    obstacles: &[Segment],
    owner: Option<u64>,
    origin: Vec2,
    direction: Vec2,
) -> f32 {
    let distance = contacts(obstacles, owner, origin, direction, true)
        .map(|(hit, segment)| match segment.owner {
            Some(_) => hit,
            None => hit / ARENA_WALL_GRIND,
        })
        .fold(f32::INFINITY, f32::min);
    (1.0 - distance / SLIDE_DISTANCE).max(0.0)
}

/// The mutable kinematic state of one rider entering a tick.
#[derive(Clone, Copy, Debug)]
pub struct RiderState {
    pub position: Vec2,
    pub direction: Vec2,
    pub target_speed: f32,
    pub rubber: f32,
}

/// The resulting state and collision flags after one tick.
pub struct StepOutcome {
    pub position: Vec2,
    pub direction: Vec2,
    pub target_speed: f32,
    pub speed: f32,
    pub rubber: f32,
    pub step: f32,
    pub sliding: bool,
    pub colliding: bool,
    pub died: bool,
}

/// Turn → sense → move → rubber for one rider against immutable start-of-tick
/// obstacles. Mutates `trail` in place (turn vertex + trimmed advance).
pub fn step_rider(
    owner: Option<u64>,
    state: RiderState,
    turn: PlayerInput,
    obstacles: &[Segment],
    trail: &mut Trail,
) -> StepOutcome {
    let RiderState {
        position,
        mut direction,
        mut target_speed,
        mut rubber,
    } = state;
    if trail.0.is_empty() {
        *trail = Trail::new(position);
    }
    match turn {
        PlayerInput::None => {}
        PlayerInput::TurnLeft | PlayerInput::TurnLeftFor(_) => {
            trail.turn(position);
            direction = rotate_left(direction);
        }
        PlayerInput::TurnRight | PlayerInput::TurnRightFor(_) => {
            trail.turn(position);
            direction = rotate_right(direction);
        }
    }
    let origin = position;
    let heading = direction;
    let front = nearest(obstacles, owner, origin, heading, false);
    let cruise = target_speed;
    let normal_step = BASE_SPEED * cruise * TICK_SECS;
    let colliding = front < SLOW_DOWN_DISTANCE;
    let step = if colliding {
        let ratio = (front / SLOW_DOWN_DISTANCE).powi(2);
        let load = 1.0 + cruise;
        rubber -= RUBBER_DRAIN * load * load * load;
        // Zeno decay may never exceed cruise speed: at the zone edge it would
        // otherwise lurch the rider forward (~12 u in one tick at base speed).
        (front * ratio)
            .min(normal_step)
            .min((front - EPSILON).max(0.0))
    } else {
        rubber = (rubber + RUBBER_REGEN).min(BASE_RUBBER);
        // Swept sensing: even arbitrarily high slide speeds cannot tunnel.
        normal_step.min((front - EPSILON).max(0.0))
    };
    rubber = rubber.clamp(0.0, BASE_RUBBER);
    if rubber == 0.0 {
        return StepOutcome {
            position,
            direction,
            target_speed: 0.0,
            speed: 0.0,
            rubber: 0.0,
            step: 0.0,
            sliding: false,
            colliding: false,
            died: true,
        };
    }
    // Inertia: cruise speed chases a grind-dependent goal at bounded rates.
    // Everything here is + - * / only, so it is bit-identical across platforms.
    let grind = grind_proximity(obstacles, owner, origin, rotate_left(heading)).max(
        grind_proximity(obstacles, owner, origin, rotate_right(heading)),
    );
    let sliding = grind > 0.0;
    let goal = 1.0 + (MAX_SPEED_MULT - 1.0) * grind;
    target_speed = if colliding {
        (cruise - CRASH_BRAKE).max(1.0)
    } else if cruise < goal {
        (cruise + SLIDE_ACCEL).min(goal)
    } else {
        (cruise - COAST_DECEL).max(goal)
    };
    let speed = step / (BASE_SPEED * TICK_SECS);
    let position = position + heading * step;
    trail.advance(position, TRAIL_MAX_LENGTH);
    StepOutcome {
        position,
        direction,
        target_speed,
        speed,
        rubber,
        step,
        sliding,
        colliding,
        died: false,
    }
}
