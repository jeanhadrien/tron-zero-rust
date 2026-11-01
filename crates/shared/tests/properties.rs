//! Property tests for the shared sim: random arenas, riders and turn scripts
//! driven through the real `simulate_players` system, checked against
//! invariants from `docs/core-gameplay-mechanics.md` with an independent
//! geometric oracle (no reuse of `physics::nearest` / `ray_hit`).

use bevy_ecs::prelude::*;
use glam::Vec2;
use proptest::prelude::*;
use tron_zero_shared::spawn::{find_safe_spawn, spawn_is_safe};
use tron_zero_shared::*;

const TICKS: usize = 240;
const CARDINALS: [Vec2; 4] = [Vec2::X, Vec2::Y, Vec2::NEG_X, Vec2::NEG_Y];

#[derive(Clone, Debug)]
struct RiderPlan {
    position: Vec2,
    direction: Vec2,
    target_speed: f32,
    /// One entry per tick: 0 = hold, 1 = left, 2 = right.
    turns: Vec<u8>,
}

fn point() -> impl Strategy<Value = Vec2> {
    (-1100.0f32..1100.0, -1100.0f32..1100.0).prop_map(|(x, y)| Vec2::new(x, y))
}

/// Axis-aligned interior wall, like trails and the arena boundary.
fn wall() -> impl Strategy<Value = [f32; 4]> {
    (point(), any::<bool>(), 20.0f32..600.0).prop_map(|(a, horizontal, length)| {
        let b = if horizontal {
            a + Vec2::X * length
        } else {
            a + Vec2::Y * length
        };
        [a.x, a.y, b.x, b.y]
    })
}

fn rider() -> impl Strategy<Value = RiderPlan> {
    let turn = prop_oneof![12 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8)];
    (
        point(),
        0usize..4,
        // Mostly realistic boosts, plus speeds whose step exceeds the 12-unit
        // rubber zone, the only regime where swept sensing prevents tunnelling.
        prop_oneof![3 => 1.0f32..3.0, 1 => 4.0f32..40.0],
        prop::collection::vec(turn, TICKS),
    )
        .prop_map(|(position, dir, target_speed, turns)| RiderPlan {
            position,
            direction: CARDINALS[dir],
            target_speed,
            turns,
        })
}

/// Independent oracle: does the move `p → q` properly cross segment `a–b`?
/// Touching at the move's start (own corners, contact) does not count, and
/// neither do parallel overlaps.
fn crosses(p: Vec2, q: Vec2, a: Vec2, b: Vec2) -> bool {
    let r = q - p;
    let s = b - a;
    let denom = r.perp_dot(s);
    if denom.abs() < 1e-6 {
        return false;
    }
    let t = (a - p).perp_dot(s) / denom;
    let u = (a - p).perp_dot(r) / denom;
    t > 1e-3 && t <= 1.0 && (-1e-4..=1.0 + 1e-4).contains(&u)
}

fn axis_aligned(a: Vec2, b: Vec2) -> bool {
    (a.x - b.x).abs() < 1e-3 || (a.y - b.y).abs() < 1e-3
}

struct Before {
    alive: bool,
    position: Vec2,
    target_speed: f32,
}

fn run_and_check(walls: Vec<[f32; 4]>, plans: Vec<RiderPlan>) -> Result<(), TestCaseError> {
    let mut world = World::new();
    world.insert_resource(SimulationRole::Server);
    let mut all_walls = arena_walls(ARENA_WIDTH, ARENA_HEIGHT);
    all_walls.extend(walls);
    world.spawn((Arena, WallSegments(all_walls.clone())));
    let mut schedule = Schedule::default();
    schedule.add_systems(simulate_players);

    let riders: Vec<Entity> = plans
        .iter()
        .map(|plan| {
            world
                .spawn((
                    Player,
                    Position(plan.position),
                    Direction(plan.direction),
                    TargetSpeedMult(plan.target_speed),
                    IsAlive(true),
                    ShouldHandleDeath(true),
                    Trail::new(plan.position),
                ))
                .id()
        })
        .collect();
    let wall_segments: Vec<[Vec2; 2]> = all_walls
        .iter()
        .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
        .collect();

    for tick in 0..TICKS {
        // Start-of-tick obstacles: walls plus every living trail.
        let mut obstacles = wall_segments.clone();
        let mut before = Vec::with_capacity(riders.len());
        for (&entity, plan) in riders.iter().zip(&plans) {
            let alive = world.get::<IsAlive>(entity).unwrap().0;
            if alive {
                let trail = &world.get::<Trail>(entity).unwrap().0;
                obstacles.extend(trail.windows(2).map(|w| [w[0], w[1]]));
            }
            before.push(Before {
                alive,
                position: world.get::<Position>(entity).unwrap().0,
                target_speed: world.get::<TargetSpeedMult>(entity).unwrap().0,
            });
            // Inputs are one-shot: every tick without a turn must hold.
            world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = match plan.turns[tick] {
                1 => PlayerInput::TurnLeft,
                2 => PlayerInput::TurnRight,
                _ => PlayerInput::None,
            };
        }

        schedule.run(&mut world);

        for (&entity, before) in riders.iter().zip(&before) {
            let alive = world.get::<IsAlive>(entity).unwrap().0;
            let rubber = world.get::<Rubber>(entity).unwrap().0;
            let trail = &world.get::<Trail>(entity).unwrap().0;
            prop_assert!(
                (0.0..=BASE_RUBBER).contains(&rubber),
                "tick {tick}: rubber {rubber}"
            );
            if !alive {
                prop_assert!(trail.is_empty(), "tick {tick}: dead rider kept its trail");
                prop_assert_eq!(rubber, 0.0);
                continue;
            }
            prop_assert!(before.alive, "tick {tick}: rider came back to life");
            let position = world.get::<Position>(entity).unwrap().0;
            let direction = world.get::<Direction>(entity).unwrap().0;
            let speed = world.get::<SpeedMult>(entity).unwrap().0;
            let colliding = world.get::<IsColliding>(entity).unwrap().0;
            prop_assert!(
                CARDINALS.contains(&direction),
                "tick {tick}: heading {direction}"
            );
            // Inside the rubber zone the Zeno step is `front³ / 12²` (JS
            // parity), which is NOT capped by target speed: entering the zone
            // can lunge up to 4x a base step. Only open-space speed is capped.
            prop_assert!(
                colliding || speed <= before.target_speed + 1e-4,
                "tick {tick}: speed {speed} above target {}",
                before.target_speed
            );
            prop_assert!(
                position.distance(before.position)
                    < SLOW_DOWN_DISTANCE.max(BASE_SPEED * before.target_speed * TICK_SECS + 1e-3),
                "tick {tick}: moved {} in one tick",
                position.distance(before.position)
            );
            prop_assert_eq!(trail.last().copied(), Some(position));
            let length: f32 = trail.windows(2).map(|w| w[0].distance(w[1])).sum();
            prop_assert!(
                length <= TRAIL_MAX_LENGTH + 0.01,
                "tick {tick}: trail {length}"
            );
            for w in trail.windows(2) {
                prop_assert!(
                    axis_aligned(w[0], w[1]),
                    "tick {tick}: diagonal trail {w:?}"
                );
            }
            for [a, b] in &obstacles {
                prop_assert!(
                    !crosses(before.position, position, *a, *b),
                    "tick {tick}: moved {} -> {position} through {a}–{b}",
                    before.position
                );
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn sim_preserves_gameplay_invariants(
        walls in prop::collection::vec(wall(), 0..5),
        plans in prop::collection::vec(rider(), 1..4),
    ) {
        run_and_check(walls, plans)?;
    }

    #[test]
    fn safe_spawn_is_safe_and_seed_deterministic(
        segments in prop::collection::vec((point(), point()), 0..20),
        riders in prop::collection::vec(point(), 0..6),
        seed in any::<u64>(),
    ) {
        let size = ArenaSize::default();
        let mut obstacles: Vec<[Vec2; 2]> = arena_walls(size.width, size.height)
            .into_iter()
            .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
            .collect();
        obstacles.extend(segments.into_iter().map(|(a, b)| [a, b]));
        let spawn = find_safe_spawn(size, &obstacles, &riders, seed);
        if let Some(spawn) = spawn {
            prop_assert!(spawn_is_safe(spawn, size, &obstacles, &riders));
        }
        prop_assert_eq!(spawn, find_safe_spawn(size, &obstacles, &riders, seed));
    }
}
