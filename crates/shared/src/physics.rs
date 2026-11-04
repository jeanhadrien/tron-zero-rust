//! Pure lightcycle physics shared by the authoritative simulation and
//! server-side forecasting, so prediction cannot drift from the sim.
//!
//! `step_rider` is a transaction over plain data: it never touches ECS. The
//! sim reads the resulting state back into components; the forecaster applies
//! it to a `WorldSnapshot`.

use glam::Vec2;

use crate::*;

const EPSILON: f32 = 0.001;
/// How far short of another rider's new trail a crossing stops a rider. It
/// must stay well outside `ray_hit`'s `EPSILON` bands, or a turn from there
/// would read as collinear with that trail and could never escape.
const STOP_MARGIN: f32 = 10.0 * EPSILON;

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
    // `along` is a fraction of the edge, so the endpoint slack is scaled back
    // to world units: a long straight must not reach past its corner.
    let slack = EPSILON / edge.length();
    if distance >= -EPSILON && (-slack..=1.0 + slack).contains(&along) {
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

/// Nearest obstacle along a ray: its distance and its owner (`None` for an
/// arena wall). The distance is the same `min` fold as `nearest`, so it is
/// bit-identical. On an exact distance tie a foreign trail wins over the
/// rider's own trail, which wins over a wall; foreign trails tie-break on the
/// lowest owner bits. That keeps the killer deterministic on both sides.
fn nearest_contact(
    obstacles: &[Segment],
    owner: Option<u64>,
    origin: Vec2,
    direction: Vec2,
) -> (f32, Option<u64>) {
    let priority = |segment_owner: Option<u64>| match segment_owner {
        Some(bits) if Some(bits) != owner => (0, bits),
        Some(bits) => (1, bits),
        None => (2, 0),
    };
    let mut best = (f32::INFINITY, None);
    for (hit, segment) in contacts(obstacles, owner, origin, direction, false) {
        if hit < best.0 || (hit == best.0 && priority(segment.owner) < priority(best.1)) {
            best = (hit, segment.owner);
        }
    }
    best
}

/// Grind strength in `0..=1` for a side ray: 1 touching an obstacle, falling
/// linearly to 0 at `SLIDE_DISTANCE`. Grinding rewards risk near *other*
/// riders' trails, so:
/// - the rider's own trail never counts (a U-turn corridor is a free farm);
/// - arena walls pay at most `ARENA_WALL_GRIND` of the full strength, so edge
///   riding is a weak accelerator even at contact.
fn grind_proximity(
    obstacles: &[Segment],
    owner: Option<u64>,
    origin: Vec2,
    direction: Vec2,
) -> f32 {
    contacts(obstacles, owner, origin, direction, true)
        .filter(|(_, segment)| owner.is_none() || segment.owner != owner)
        .map(|(hit, segment)| {
            let strength = (1.0 - hit / SLIDE_DISTANCE).max(0.0);
            match segment.owner {
                Some(_) => strength,
                None => strength * ARENA_WALL_GRIND,
            }
        })
        .fold(0.0, f32::max)
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
    /// Owner of the nearest obstacle ahead this tick (`None` = arena wall;
    /// the rider's own bits = its own trail). Rubber only drains against this
    /// contact, so on a death it is the cause. Meaningless when nothing is ahead.
    pub front_owner: Option<u64>,
}

/// Turn → sense → move → rubber for one rider against immutable start-of-tick
/// obstacles. Mutates `trail` in place (turn vertex + trimmed advance).
pub(crate) fn step_rider(
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
    let (front, front_owner) = nearest_contact(obstacles, owner, origin, heading);
    let cruise = target_speed;
    let normal_step = BASE_SPEED * cruise * TICK_SECS;
    let colliding = front < SLOW_DOWN_DISTANCE;
    let step = if colliding {
        let fraction = front / SLOW_DOWN_DISTANCE;
        let ratio = fraction * fraction;
        rubber -= crash_drain(cruise);
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
            front_owner,
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
        crash_brake(cruise)
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
        front_owner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(a: Vec2, b: Vec2, owner: Option<u64>) -> Segment {
        Segment {
            a,
            b,
            owner,
            from_head: 5,
        }
    }

    /// The owner-aware search must report exactly the distance `nearest` does.
    #[test]
    fn nearest_contact_distance_matches_nearest() {
        let obstacles = [
            segment(Vec2::new(50.0, -100.0), Vec2::new(50.0, 100.0), Some(7)),
            segment(Vec2::new(30.0, -100.0), Vec2::new(30.0, 100.0), None),
            segment(Vec2::new(90.0, -100.0), Vec2::new(90.0, 100.0), Some(9)),
        ];
        let (distance, owner) = nearest_contact(&obstacles, Some(1), Vec2::ZERO, Vec2::X);
        assert_eq!(
            distance.to_bits(),
            nearest(&obstacles, Some(1), Vec2::ZERO, Vec2::X, false).to_bits()
        );
        assert_eq!(owner, None);
    }

    /// At an exact distance tie a foreign trail beats a wall and the rider's
    /// own trail; between foreign trails the lowest owner bits win.
    #[test]
    fn nearest_contact_breaks_ties_toward_foreign_trails() {
        let wall = segment(Vec2::new(40.0, -100.0), Vec2::new(40.0, 100.0), None);
        let own = segment(Vec2::new(40.0, -100.0), Vec2::new(40.0, 100.0), Some(1));
        let foreign_high = segment(Vec2::new(40.0, -100.0), Vec2::new(40.0, 100.0), Some(9));
        let foreign_low = segment(Vec2::new(40.0, -100.0), Vec2::new(40.0, 100.0), Some(3));
        let obstacles = [wall, own, foreign_high, foreign_low];
        let (_, owner) = nearest_contact(&obstacles, Some(1), Vec2::ZERO, Vec2::X);
        assert_eq!(owner, Some(3));
    }
}

/// One rider entering a tick: who it is (for self-trail exclusions), its
/// state, the turn it executes and its trail, which the tick updates in place.
pub struct RiderStep {
    pub owner: Option<u64>,
    pub state: RiderState,
    pub turn: PlayerInput,
    pub trail: Trail,
}

/// The whole tick for every live rider: `step_rider` each one against the
/// start-of-tick `obstacles`, then `resolve_crossings` between this tick's
/// moves. The authoritative sim and the bot forecaster both call this, so a
/// projected world cannot drift from the real one. Returns one outcome per
/// rider, in order; a `died` rider's trail is the caller's to clear.
pub fn step_riders(riders: &mut [RiderStep], obstacles: &[Segment]) -> Vec<StepOutcome> {
    let mut outcomes: Vec<StepOutcome> = riders
        .iter_mut()
        .map(|r| step_rider(r.owner, r.state, r.turn, obstacles, &mut r.trail))
        .collect();
    let moves: Vec<Move> = riders
        .iter()
        .zip(&outcomes)
        .map(|(r, o)| Move {
            from: r.state.position,
            direction: o.direction,
            // A dead rider does not move; a zero move never meets anything.
            length: if o.died { 0.0 } else { o.step },
        })
        .collect();
    for (i, length) in resolve_crossings(&moves).into_iter().enumerate() {
        let Some(length) = length else {
            continue;
        };
        // Being stopped is a collision: it costs rubber like facing a wall,
        // or two riders tied at a crossing would wait there forever.
        let (rider, outcome, m) = (&mut riders[i], &mut outcomes[i], moves[i]);
        let (target_speed, rubber) = crash(rider.state.target_speed, rider.state.rubber);
        if rubber == 0.0 {
            outcome.died = true;
            continue;
        }
        outcome.position = m.from + m.direction * length;
        outcome.step = length;
        outcome.speed = length / (BASE_SPEED * TICK_SECS);
        outcome.target_speed = target_speed;
        outcome.rubber = rubber;
        outcome.sliding = false;
        outcome.colliding = true;
        rider.trail.advance(outcome.position, TRAIL_MAX_LENGTH);
    }
    outcomes
}

/// Rubber one tick stuck facing an obstacle costs at cruise multiplier `cruise`.
fn crash_drain(cruise: f32) -> f32 {
    let load = 1.0 + cruise;
    RUBBER_DRAIN * load * load * load
}

/// Cruise multiplier after one tick stuck facing an obstacle.
fn crash_brake(cruise: f32) -> f32 {
    (cruise - CRASH_BRAKE).max(1.0)
}

/// `(target_speed, rubber)` for a rider that entered the tick with `cruise`
/// and `rubber` and was stopped by `resolve_crossings`: the same cost as a
/// tick spent facing an obstacle, replacing whatever `step_rider` decided.
fn crash(cruise: f32, rubber: f32) -> (f32, f32) {
    (
        crash_brake(cruise),
        (rubber - crash_drain(cruise)).clamp(0.0, BASE_RUBBER),
    )
}

/// One rider's straight movement during a tick: `length` units along
/// `direction` (unit, cardinal) from `from`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Move {
    pub from: Vec2,
    pub direction: Vec2,
    pub length: f32,
}

/// Two moves meeting at `time` (fraction of the tick): `arrive[k]` is when
/// `riders[k]` reaches the meeting point, and `blocks[k]` whether it is the
/// one stopped there.
struct Meeting {
    time: f32,
    riders: [usize; 2],
    arrive: [f32; 2],
    blocks: [bool; 2],
}

/// Sweeps this tick's moves against each other: geometry grown during the tick
/// is invisible to the start-of-tick snapshot `step_rider` senses against.
///
/// Whoever reaches a crossing first keeps going and the later rider stops just
/// short of the new trail; equal arrivals and head-on meetings stop both. A
/// rider that was itself stopped before a crossing never lays trail there.
/// Returns each move's new length when it was shortened. Order-independent,
/// and `+ - * /` only like the rest of the sim.
pub(crate) fn resolve_crossings(moves: &[Move]) -> Vec<Option<f32>> {
    let mut meetings = Vec::new();
    for i in 0..moves.len() {
        for j in i + 1..moves.len() {
            meetings.extend(meeting(moves, i, j));
        }
    }
    // Stops happen in time order, so each meeting sees every earlier stop.
    // Meetings at the same time commute (a stop at `t` only matters for
    // arrivals after `t`), so their relative order is irrelevant.
    meetings.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut stops: Vec<Option<f32>> = vec![None; moves.len()];
    let reaches = |stops: &[Option<f32>], rider: usize, time: f32| {
        stops[rider].is_none_or(|stop| stop > time)
    };
    for m in &meetings {
        if !(0..2).all(|k| reaches(&stops, m.riders[k], m.arrive[k])) {
            continue;
        }
        for k in (0..2).filter(|&k| m.blocks[k]) {
            stops[m.riders[k]] = Some(m.time);
        }
    }
    stops
        .iter()
        .zip(moves)
        .map(|(stop, m)| stop.map(|t| (m.length * t - STOP_MARGIN).max(0.0)))
        .collect()
}

/// Where and when moves `i` and `j` meet, if their new segments touch.
fn meeting(moves: &[Move], i: usize, j: usize) -> Option<Meeting> {
    let (a, b) = (moves[i], moves[j]);
    if a.length <= 0.0 || b.length <= 0.0 {
        // A rider that did not move is a point already in the snapshot.
        return None;
    }
    let offset = b.from - a.from;
    let cross = a.direction.perp_dot(b.direction);
    if cross.abs() <= EPSILON {
        // Head-on along one line: they close the gap together.
        let gap = offset.dot(a.direction);
        let closing = a.length + b.length;
        let collinear = offset.perp_dot(a.direction).abs() <= EPSILON;
        if !collinear || a.direction.dot(b.direction) >= 0.0 || gap <= 0.0 || closing < gap {
            return None;
        }
        let time = gap / closing;
        return Some(Meeting {
            time,
            riders: [i, j],
            arrive: [time, time],
            blocks: [true, true],
        });
    }
    // Perpendicular: each rider's distance to the crossing point.
    let along_a = offset.perp_dot(b.direction) / cross;
    let along_b = offset.perp_dot(a.direction) / cross;
    if !(0.0..=a.length).contains(&along_a) || !(0.0..=b.length).contains(&along_b) {
        return None;
    }
    let arrive = [along_a / a.length, along_b / b.length];
    Some(Meeting {
        time: arrive[0].max(arrive[1]),
        riders: [i, j],
        arrive,
        blocks: [arrive[0] >= arrive[1], arrive[1] >= arrive[0]],
    })
}
