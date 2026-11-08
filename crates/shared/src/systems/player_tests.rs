use super::*;
use crate::physics::ray_hit;
use bevy_ecs::resource::IsResource;
use lightyear::prelude::{HistoryState, Tick};

fn setup() -> (World, Schedule) {
    let mut world = World::new();
    world.insert_resource(SimulationRole::Server);
    world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
    let mut schedule = Schedule::default();
    schedule.add_systems(simulate_players);
    (world, schedule)
}

fn cycle(world: &mut World, position: Vec2, direction: Vec2) -> Entity {
    world
        .spawn((
            Player,
            Position(position),
            Direction(direction),
            SpeedMult::base(),
            IsAlive(true),
            ShouldHandleDeath(true),
            Trail::new(position),
        ))
        .id()
}

fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 0.01, "{a} != {b}");
}

/// Gameplay entities only: Bevy 0.19 stores resources as entities too.
fn sim_entities(world: &mut World) -> usize {
    world
        .query_filtered::<(), Without<IsResource>>()
        .iter(world)
        .count()
}

fn trail_length(trail: &Trail, head: Vec2) -> f32 {
    crate::polyline::length(&trail.points(head))
}

/// Point-wise `near` for polylines: trail trimming accumulates float error.
fn near_points(actual: &[Vec2], expected: &[Vec2]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?} != {expected:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert!(a.distance(*e) < 0.01, "{actual:?} != {expected:?}");
    }
}

#[test]
fn cardinal_velocity_preserves_360_units_per_second() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    schedule.run(&mut world);
    near(world.get::<Position>(entity).unwrap().0.x, 3.0);
    near(world.get::<Velocity>(entity).unwrap().0.x, 3000.0);
    near(
        trail_length(world.get::<Trail>(entity).unwrap(), Vec2::X * 3.0),
        3.0,
    );
}

#[test]
fn human_turns_are_scoped_to_the_life_being_simulated() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    world
        .entity_mut(entity)
        .insert((LifeGeneration(2), ActionState(PlayerInput::TurnLeftFor(1))));
    schedule.run(&mut world);
    assert_eq!(world.get::<Direction>(entity).unwrap().0, Vec2::X);
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    assert_eq!(world.get::<Direction>(entity).unwrap().0, Vec2::X);
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeftFor(2);
    schedule.run(&mut world);
    assert_eq!(world.get::<Direction>(entity).unwrap().0, Vec2::Y);
    // Rewound life state accepts its own historical input, not a future life.
    world.get_mut::<LifeGeneration>(entity).unwrap().0 = 1;
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnRightFor(2);
    schedule.run(&mut world);
    assert_eq!(world.get::<Direction>(entity).unwrap().0, Vec2::Y);
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnRightFor(1);
    schedule.run(&mut world);
    assert_eq!(world.get::<Direction>(entity).unwrap().0, Vec2::X);
}

#[test]
fn ray_contacts_cover_zero_distance_collinear_and_endpoints() {
    assert_eq!(ray_hit(Vec2::ZERO, Vec2::X, Vec2::ZERO, Vec2::Y), Some(0.0));
    assert_eq!(
        ray_hit(Vec2::ZERO, Vec2::X, Vec2::NEG_X, Vec2::X),
        Some(0.0)
    );
    assert_eq!(
        ray_hit(Vec2::ZERO, Vec2::X, Vec2::X * 4.0, Vec2::X * 10.0),
        Some(4.0)
    );
    assert_eq!(
        ray_hit(Vec2::ZERO, Vec2::X, Vec2::NEG_X * 4.0, Vec2::NEG_X),
        None
    );
    assert_eq!(ray_hit(Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::Y * 4.0), None);
    assert_eq!(
        ray_hit(Vec2::ZERO, Vec2::X, Vec2::X * 4.0, Vec2::X * 4.0),
        Some(4.0)
    );
}

#[test]
fn wall_rubber_drains_then_death_clears_trail_once() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::new(ARENA_WIDTH * 0.5 - 1.0, 0.0), Vec2::X);
    world.get_mut::<Rubber>(entity).unwrap().0 = 10.0;
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<IsAlive>(entity).unwrap().0);
    // One unit from the wall the rider closes 28% of the gap and pays for the
    // rest of its 3-unit step.
    let lost = 3.0 - RUBBER_APPROACH;
    let expected = 10.0 + (10.0 + RUBBER_REGEN_FLOOR) * RUBBER_REGEN - lost * RUBBER_COST;
    near(world.get::<Rubber>(entity).unwrap().0, expected);
    for _ in 0..3 {
        schedule.run(&mut world);
    }
    assert!(!world.get::<IsAlive>(entity).unwrap().0);
    assert!(!world.get::<ShouldHandleDeath>(entity).unwrap().0);
    assert_eq!(world.get::<Rubber>(entity).unwrap().0, 0.0);
    assert_eq!(world.get::<Velocity>(entity).unwrap().0, Vec2::ZERO);
    assert!(world.get::<Trail>(entity).unwrap().0.is_empty());
    let position = world.get::<Position>(entity).unwrap().0;
    schedule.run(&mut world);
    assert_eq!(world.get::<Position>(entity).unwrap().0, position);
}

#[test]
fn entering_the_slowdown_zone_never_speeds_the_rider_up() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(
        &mut world,
        Vec2::new(ARENA_WIDTH * 0.5 - 11.9, 0.0),
        Vec2::X,
    );
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(entity).unwrap().0);
    let speed = world.get::<SpeedMult>(entity).unwrap().0;
    assert!(speed <= 1.0 + 1e-4, "lurched forward at {speed}x");
    // The edge of the zone is free: nothing was lost yet.
    assert!(world.get::<Rubber>(entity).unwrap().0 >= BASE_RUBBER - 1e-3);
}

#[test]
fn escape_regenerates_rubber_and_keeps_the_speed() {
    let (mut world, mut schedule) = setup();
    let edge = ARENA_WIDTH * 0.5;
    let entity = cycle(&mut world, Vec2::new(edge - 1.0, 0.0), Vec2::X);
    world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 2.0;
    world.get_mut::<Rubber>(entity).unwrap().0 = 70.0;
    schedule.run(&mut world);
    assert!(world.get::<SpeedMult>(entity).unwrap().0 < 1.0);
    // Facing the wall does not bleed the boost: it only fades like in open space.
    let target = world.get::<TargetSpeedMult>(entity).unwrap().0;
    near(target, 2.0 - SPEED_DECAY);
    let rubber = world.get::<Rubber>(entity).unwrap().0;
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    assert!(!world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<Rubber>(entity).unwrap().0 > rubber);
    // The turn costs 5% of the cruise speed.
    near(
        world.get::<SpeedMult>(entity).unwrap().0,
        target * TURN_SPEED_FACTOR,
    );
    assert!(world.get::<IsSliding>(entity).unwrap().0);
}

#[test]
fn rubber_regen_runs_while_grinding_and_is_proportional_to_what_is_left() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(
        &mut world,
        Vec2::new(0.0, ARENA_HEIGHT * 0.5 - 5.0),
        Vec2::X,
    );
    world.get_mut::<Rubber>(entity).unwrap().0 = 20.0;
    schedule.run(&mut world);
    assert!(world.get::<IsSliding>(entity).unwrap().0);
    near(
        world.get::<Rubber>(entity).unwrap().0,
        20.0 + (20.0 + RUBBER_REGEN_FLOOR) * RUBBER_REGEN,
    );
    // More left means faster, and the meter never exceeds its maximum.
    world.get_mut::<Rubber>(entity).unwrap().0 = 100.0;
    schedule.run(&mut world);
    near(
        world.get::<Rubber>(entity).unwrap().0,
        100.0 + (100.0 + RUBBER_REGEN_FLOOR) * RUBBER_REGEN,
    );
    world.get_mut::<Rubber>(entity).unwrap().0 = BASE_RUBBER - 0.01;
    schedule.run(&mut world);
    assert_eq!(world.get::<Rubber>(entity).unwrap().0, BASE_RUBBER);
}

/// Grind strength of a trail `d` units to the side, as the sim defines it.
fn strength_at(d: f32) -> f32 {
    crate::physics::grind_strength(d)
}

#[test]
fn grind_strength_is_convex_and_reaches_zero_at_the_range() {
    near(strength_at(0.0), 1.0);
    near(strength_at(GRIND_OFFSET), 1.0 / 3.0);
    near(strength_at(GRIND_RANGE), 0.0);
    // Touching pays far more than hovering: convex, not linear.
    assert!(strength_at(8.0) < 1.0 - 8.0 / GRIND_RANGE);
}

/// First-tick target speed of a rider with a foreign trail 3 units to each
/// listed side (`1.0` = left, `-1.0` = right).
fn first_tick_target(sides: &[f32]) -> f32 {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    for &side in sides {
        let head = Vec2::new(400.0, 3.0 * side);
        let other = cycle(&mut world, head, Vec2::X);
        *world.get_mut::<Trail>(other).unwrap() = Trail(vec![head - Vec2::X * 800.0]);
    }
    schedule.run(&mut world);
    world.get::<TargetSpeedMult>(entity).unwrap().0
}

#[test]
fn both_sides_add_up() {
    let single = first_tick_target(&[1.0]);
    let tunnel = first_tick_target(&[1.0, -1.0]);
    near(single - 1.0, strength_at(3.0) * GRIND_ACCEL);
    near(tunnel - 1.0, 2.0 * strength_at(3.0) * GRIND_ACCEL);
}

#[test]
fn grinding_converges_on_a_proximity_scaled_terminal_speed() {
    let (mut world, mut schedule) = setup();
    // Ten seconds of riding need more room than the default arena.
    let mut walls = world.query::<&mut WallSegments>();
    walls.single_mut(&mut world).unwrap().0 = arena_walls(8000.0, 8000.0);
    let edge = 4000.0;
    let entity = cycle(&mut world, Vec2::new(-edge + 100.0, edge - 6.0), Vec2::X);
    let mut last = 1.0;
    for _ in 0..(10.0 * TICK_HZ) as usize {
        schedule.run(&mut world);
        let target = world.get::<TargetSpeedMult>(entity).unwrap().0;
        assert!(target >= last - 1e-6 && target <= MAX_SPEED_MULT);
        last = target;
    }
    // Arena walls pay only ARENA_WALL_GRIND of the trail strength.
    near(
        last,
        1.0 + GRIND_ACCEL_PER_SEC / SPEED_DECAY_PER_SEC * strength_at(6.0) * ARENA_WALL_GRIND,
    );
}

#[test]
fn slide_boosts_next_tick_and_open_space_decays_exponentially() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(
        &mut world,
        Vec2::new(0.0, ARENA_HEIGHT * 0.5 - 5.0),
        Vec2::X,
    );
    schedule.run(&mut world);
    assert!(world.get::<IsSliding>(entity).unwrap().0);
    near(world.get::<SpeedMult>(entity).unwrap().0, 1.0);
    let target = world.get::<TargetSpeedMult>(entity).unwrap().0;
    assert!(target > 1.0);
    schedule.run(&mut world);
    near(world.get::<SpeedMult>(entity).unwrap().0, target);
    world.get_mut::<Position>(entity).unwrap().0 = Vec2::ZERO;
    *world.get_mut::<Trail>(entity).unwrap() = Trail::new(Vec2::ZERO);
    world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 2.0;
    schedule.run(&mut world);
    assert!(!world.get::<IsSliding>(entity).unwrap().0);
    near(
        world.get::<TargetSpeedMult>(entity).unwrap().0,
        2.0 - SPEED_DECAY,
    );
    near(world.get::<SpeedMult>(entity).unwrap().0, 2.0);
    // The excess fades by a fixed fraction per tick, not a fixed amount.
    world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 1.5;
    schedule.run(&mut world);
    near(
        world.get::<TargetSpeedMult>(entity).unwrap().0,
        1.5 - 0.5 * SPEED_DECAY,
    );
}

#[test]
fn turns_cost_speed_down_to_a_floor_and_recover_below_base() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 2.0;
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    // The penalty applies to the tick's own step.
    near(
        world.get::<SpeedMult>(entity).unwrap().0,
        2.0 * TURN_SPEED_FACTOR,
    );
    // Turn spam settles at the floor instead of stalling the rider.
    world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 1.0;
    for i in 0..100 {
        let turn = if i % 2 == 0 {
            PlayerInput::TurnRight
        } else {
            PlayerInput::TurnLeft
        };
        world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = turn;
        schedule.run(&mut world);
        let target = world.get::<TargetSpeedMult>(entity).unwrap().0;
        assert!(world.get::<IsAlive>(entity).unwrap().0, "died at tick {i}");
        assert!(target >= MIN_SPEED_MULT - 0.05, "stalled at {target}");
    }
    // Back on a straight, it recovers to base speed.
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::None;
    for i in 0..(1.5 * TICK_HZ) as usize {
        schedule.run(&mut world);
        let p = world.get::<Position>(entity).unwrap().0;
        assert!(
            world.get::<IsAlive>(entity).unwrap().0,
            "died in recovery tick {i} at {p}"
        );
    }
    near(world.get::<TargetSpeedMult>(entity).unwrap().0, 1.0);
}

#[test]
fn opponent_active_trail_can_be_a_sliding_wall() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    let other = cycle(&mut world, Vec2::new(100.0, 5.0), Vec2::Y);
    *world.get_mut::<Trail>(other).unwrap() = Trail(vec![Vec2::new(-10.0, 5.0)]);
    schedule.run(&mut world);
    assert!(world.get::<IsSliding>(entity).unwrap().0);
    assert!(!world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<TargetSpeedMult>(entity).unwrap().0 > 1.0);
}

#[test]
fn exact_boundary_contact_blocks_outward_but_allows_escape_or_tangent() {
    for heading in [Vec2::X, Vec2::NEG_X, Vec2::Y] {
        let (mut world, mut schedule) = setup();
        let entity = cycle(&mut world, Vec2::new(ARENA_WIDTH * 0.5, 0.0), heading);
        schedule.run(&mut world);
        let blocked = heading == Vec2::X;
        assert_eq!(world.get::<IsColliding>(entity).unwrap().0, blocked);
        if blocked {
            assert_eq!(world.get::<Velocity>(entity).unwrap().0, Vec2::ZERO);
            assert!(world.get::<Rubber>(entity).unwrap().0 < BASE_RUBBER);
        } else {
            assert!(world.get::<Velocity>(entity).unwrap().0.length() > 0.0);
        }
    }
}

#[test]
fn opponent_active_and_completed_segments_both_collide() {
    for points in [
        vec![Vec2::new(5.0, -10.0), Vec2::new(5.0, 10.0)],
        vec![
            Vec2::new(5.0, -10.0),
            Vec2::new(5.0, 10.0),
            Vec2::new(20.0, 10.0),
        ],
    ] {
        let (mut world, mut schedule) = setup();
        let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
        let head = *points.last().unwrap();
        let other = cycle(&mut world, head, Vec2::Y);
        *world.get_mut::<Trail>(other).unwrap() = Trail(points[..points.len() - 1].to_vec());
        schedule.run(&mut world);
        assert!(world.get::<IsColliding>(entity).unwrap().0);
        assert!(world.get::<Position>(entity).unwrap().0.x < 5.0);
        assert!(world.get::<Rubber>(entity).unwrap().0 < BASE_RUBBER);
    }
}

#[test]
fn old_self_segment_collides_but_own_active_and_turn_junction_do_not() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![
        Vec2::new(5.0, -10.0),
        Vec2::new(5.0, 10.0),
        Vec2::new(-10.0, 10.0),
        Vec2::new(-10.0, 0.0),
    ]);
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(entity).unwrap().0);

    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![Vec2::NEG_X * 10.0]);
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    near(world.get::<Position>(entity).unwrap().0.x, 0.0);
    near(
        world.get::<Position>(entity).unwrap().0.y,
        3.0 * TURN_SPEED_FACTOR,
    );
    assert!(!world.get::<IsColliding>(entity).unwrap().0);
    assert!(!world.get::<IsSliding>(entity).unwrap().0);
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::None;
    schedule.run(&mut world);
    assert!(!world.get::<IsSliding>(entity).unwrap().0);
}

#[test]
fn reversing_into_own_active_segment_is_not_excluded() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::NEG_X);
    *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![Vec2::NEG_X * 10.0]);
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(entity).unwrap().0);
    assert_eq!(world.get::<Position>(entity).unwrap().0, Vec2::ZERO);
}

#[test]
fn high_speed_cannot_tunnel_through_wall_or_trail() {
    for trail in [false, true] {
        let (mut world, mut schedule) = setup();
        let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
        let barrier = if trail {
            let other = cycle(&mut world, Vec2::new(50.0, 10.0), Vec2::Y);
            *world.get_mut::<Trail>(other).unwrap() = Trail(vec![Vec2::new(50.0, -10.0)]);
            50.0
        } else {
            1200.0
        };
        world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 1000.0;
        schedule.run(&mut world);
        assert!(world.get::<Position>(entity).unwrap().0.x < barrier);
        schedule.run(&mut world);
        assert!(world.get::<Position>(entity).unwrap().0.x < barrier);
    }
}

#[test]
fn cornered_trail_consumes_oldest_end_gradually() {
    // Zigzag corners 10 apart, then a long run: the tail eats the corners one
    // by one and the trail stays at the cap.
    let mut corners = vec![Vec2::ZERO];
    for i in 1..=4 {
        let step = if i % 2 == 1 { Vec2::X } else { Vec2::Y };
        corners.push(corners[i - 1] + step * 10.0);
    }
    let mut trail = Trail(corners);
    let last = Vec2::new(20.0, 20.0);
    let head = last + Vec2::Y * (TRAIL_MAX_LENGTH - 15.0);
    trail.prune(head);
    near_points(
        &trail.points(head),
        &[Vec2::new(15.0, 10.0), Vec2::new(20.0, 10.0), last, head],
    );
    near(trail_length(&trail, head), TRAIL_MAX_LENGTH);
    let head = last + Vec2::Y * (TRAIL_MAX_LENGTH + 20.0);
    trail.prune(head);
    assert_eq!(trail.0, vec![last]);
    near_points(&trail.points(head), &[last + Vec2::Y * 20.0, head]);
}

#[test]
fn simulation_caps_long_straight_trail_without_erasing_active_segment() {
    let (mut world, mut schedule) = setup();
    // The trail cap exceeds the default arena's width: ride in a bigger one.
    let mut walls = world.query::<&mut WallSegments>();
    walls.single_mut(&mut world).unwrap().0 = arena_walls(8000.0, 8000.0);
    let entity = cycle(&mut world, Vec2::new(-3000.0, 0.0), Vec2::X);
    for _ in 0..(TRAIL_MAX_LENGTH / 3.0) as usize + 50 {
        schedule.run(&mut world);
    }
    let trail = world.get::<Trail>(entity).unwrap();
    let head = world.get::<Position>(entity).unwrap().0;
    near(trail_length(trail, head), TRAIL_MAX_LENGTH);
    // A straight run never adds corners: the replicated value stays put.
    assert_eq!(trail.0, vec![Vec2::new(-3000.0, 0.0)]);
    let points = trail.points(head);
    assert_eq!(points.len(), 2);
    assert!(points[0].distance(points[1]) > 0.0);
    assert!(world.get::<IsAlive>(entity).unwrap().0);
}

/// Replication sends what change detection flags: a rider riding straight
/// must leave its trail and heading unflagged, or they go out every tick.
#[test]
fn riding_straight_flags_neither_trail_nor_heading() {
    #[derive(Resource, Default)]
    struct Flagged(u32, u32);
    fn count(
        trails: Query<(), Changed<Trail>>,
        headings: Query<(), Changed<Direction>>,
        mut flagged: ResMut<Flagged>,
    ) {
        flagged.0 += trails.iter().count() as u32;
        flagged.1 += headings.iter().count() as u32;
    }
    let (mut world, mut schedule) = setup();
    world.init_resource::<Flagged>();
    schedule.add_systems(count.after(simulate_players));
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    schedule.run(&mut world); // the spawn itself counts as a change
    *world.resource_mut::<Flagged>() = Flagged::default();
    for _ in 0..10 {
        schedule.run(&mut world);
    }
    assert_eq!(
        (world.resource::<Flagged>().0, world.resource::<Flagged>().1),
        (0, 0)
    );
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    assert_eq!(
        (world.resource::<Flagged>().0, world.resource::<Flagged>().1),
        (1, 1)
    );
}

#[test]
fn zero_length_turns_do_not_churn_vertices() {
    let mut trail = Trail::new(Vec2::ZERO);
    for _ in 0..10 {
        trail.turn(Vec2::ZERO);
    }
    assert_eq!(trail.0.len(), 1);
    trail.turn(Vec2::X * 3.0);
    trail.turn(Vec2::X * 3.0);
    assert_eq!(trail.0, vec![Vec2::ZERO, Vec2::X * 3.0]);
}

#[test]
fn dead_opponents_are_not_obstacles() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    let other = cycle(&mut world, Vec2::new(1.0, 10.0), Vec2::Y);
    *world.get_mut::<Trail>(other).unwrap() = Trail(vec![Vec2::new(1.0, -10.0)]);
    world.get_mut::<IsAlive>(other).unwrap().0 = false;
    schedule.run(&mut world);
    assert!(!world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<Trail>(other).unwrap().0.is_empty());
}

#[test]
fn client_advances_only_predicted_player_and_has_no_trail_child_entities() {
    let (mut world, mut schedule) = setup();
    world.insert_resource(SimulationRole::Client);
    let local = cycle(&mut world, Vec2::ZERO, Vec2::X);
    world.entity_mut(local).insert(Predicted);
    let remote = cycle(&mut world, Vec2::new(100.0, 100.0), Vec2::Y);
    world.get_mut::<ActionState<PlayerInput>>(remote).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    near(world.get::<Position>(local).unwrap().0.x, 3.0);
    near(world.get::<Position>(local).unwrap().0.y, 0.0);
    assert_eq!(
        world.get::<Position>(remote).unwrap().0,
        Vec2::new(100.0, 100.0)
    );
    assert_eq!(world.get::<Direction>(remote).unwrap().0, Vec2::Y);
    assert_eq!(sim_entities(&mut world), 3);
}

#[test]
fn client_rollback_uses_historical_opponent_trails_not_future_render_state() {
    let (mut world, mut schedule) = setup();
    world.insert_resource(SimulationRole::Client);
    let mut timeline = LocalTimeline::default();
    timeline.apply_delta(11);
    world.insert_resource(timeline);
    let local = cycle(&mut world, Vec2::ZERO, Vec2::X);
    world.entity_mut(local).insert(Predicted);
    let remote = cycle(&mut world, Vec2::new(100.0, 100.0), Vec2::Y);
    let mut trails = ConfirmedHistory::<Trail>::default();
    trails.insert_explicit(
        Tick(10),
        HistoryState::Updated(Trail(vec![Vec2::new(5.0, -10.0)])),
    );
    trails.insert_explicit(
        Tick(20),
        HistoryState::Updated(Trail::new(Vec2::new(100.0, 100.0))),
    );
    let mut heads = ConfirmedHistory::<Position>::default();
    heads.insert_explicit(
        Tick(10),
        HistoryState::Updated(Position(Vec2::new(5.0, 10.0))),
    );
    heads.insert_explicit(
        Tick(20),
        HistoryState::Updated(Position(Vec2::new(100.0, 100.0))),
    );
    let mut lives = ConfirmedHistory::<IsAlive>::default();
    lives.insert_explicit(Tick(10), HistoryState::Updated(IsAlive(true)));
    world.entity_mut(remote).insert((trails, heads, lives));
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(local).unwrap().0);
    assert!(world.get::<Position>(local).unwrap().0.x < 5.0);
}

#[test]
fn restoring_trail_snapshot_replays_turn_without_duplicate_geometry() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    schedule.run(&mut world);
    let trail = world.get::<Trail>(entity).unwrap().clone();
    let position = *world.get::<Position>(entity).unwrap();
    let direction = *world.get::<Direction>(entity).unwrap();
    let target = *world.get::<TargetSpeedMult>(entity).unwrap();
    let rubber = *world.get::<Rubber>(entity).unwrap();
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    let result = world.get::<Trail>(entity).unwrap().clone();
    world
        .entity_mut(entity)
        .insert((trail, position, direction, target, rubber));
    schedule.run(&mut world);
    assert_eq!(*world.get::<Trail>(entity).unwrap(), result);
    assert_eq!(sim_entities(&mut world), 2);
}

#[test]
fn snapshot_collision_results_do_not_depend_on_spawn_order() {
    let run = |reverse: bool| {
        let (mut world, mut schedule) = setup();
        let mut entities = Vec::new();
        let mut positions = vec![Vec2::ZERO, Vec2::new(5.0, 10.0)];
        if reverse {
            positions.reverse();
        }
        for p in positions {
            let entity = cycle(
                &mut world,
                p,
                if p == Vec2::ZERO { Vec2::X } else { Vec2::Y },
            );
            if p != Vec2::ZERO {
                *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![Vec2::new(5.0, -10.0)]);
            }
            entities.push((p, entity));
        }
        schedule.run(&mut world);
        entities.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        entities
            .iter()
            .map(|(_, e)| {
                (
                    world.get::<Position>(*e).unwrap().0,
                    world.get::<Rubber>(*e).unwrap().0,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn seeded_spawns_are_repeatable_distinct_and_inside_centered_arena() {
    let a = spawn_from_seed(1);
    assert_eq!(a, spawn_from_seed(1));
    assert_ne!(a.0, spawn_from_seed(2).0);
    assert!(a.0.x.abs() <= ARENA_WIDTH * 0.5 - 100.0);
    assert!(a.0.y.abs() <= ARENA_HEIGHT * 0.5 - 100.0);
    assert_eq!(a.1.length_squared(), 1.0);
}

/// Two riders whose trails end at their positions, coming from 50 units back.
fn riders_with_tails(world: &mut World, riders: [(Vec2, Vec2); 2]) -> [Entity; 2] {
    riders.map(|(position, direction)| {
        let entity = cycle(world, position, direction);
        *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![position - direction * 50.0]);
        entity
    })
}

#[test]
fn same_tick_crossing_blocks_the_later_arrival() {
    let (mut world, mut schedule) = setup();
    // A reaches (0,0) a third into the tick, B two thirds in.
    let [a, b] = riders_with_tails(
        &mut world,
        [
            (Vec2::new(-1.0, 0.0), Vec2::X),
            (Vec2::new(0.0, -2.0), Vec2::Y),
        ],
    );
    schedule.run(&mut world);
    near(world.get::<Position>(a).unwrap().0.x, 2.0);
    let pb = world.get::<Position>(b).unwrap().0;
    assert!(
        pb.y < 0.0 && pb.y > -0.01,
        "B crossed or stopped early: {pb}"
    );
    assert!(world.get::<IsColliding>(b).unwrap().0);
    assert!(world.get::<Rubber>(b).unwrap().0 < BASE_RUBBER);
    for _ in 0..5 {
        schedule.run(&mut world);
    }
    assert!(world.get::<Position>(b).unwrap().0.y < 0.0);
    // Stopped just short of A's trail, B can still turn along it.
    world.get_mut::<ActionState<PlayerInput>>(b).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    world.get_mut::<ActionState<PlayerInput>>(b).unwrap().0 = PlayerInput::None;
    for _ in 0..120 {
        schedule.run(&mut world);
    }
    assert!(world.get::<IsAlive>(b).unwrap().0, "B could not turn away");
    assert!(world.get::<IsAlive>(a).unwrap().0);
}

#[test]
fn a_crossing_stop_keeps_the_full_trail_length() {
    // The tail follows the final (shortened) head, not the move B would have
    // made without A in the way.
    let (mut world, mut schedule) = setup();
    let mut walls = world.query::<&mut WallSegments>();
    walls.single_mut(&mut world).unwrap().0 = arena_walls(8000.0, 8000.0);
    let [_, b] = riders_with_tails(
        &mut world,
        [
            (Vec2::new(-1.0, 0.0), Vec2::X),
            (Vec2::new(0.0, -2.0), Vec2::Y),
        ],
    );
    *world.get_mut::<Trail>(b).unwrap() = Trail(vec![Vec2::new(0.0, -3000.0)]);
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(b).unwrap().0);
    let head = world.get::<Position>(b).unwrap().0;
    near(
        trail_length(world.get::<Trail>(b).unwrap(), head),
        TRAIL_MAX_LENGTH,
    );
}

#[test]
fn equal_arrival_at_a_crossing_blocks_both() {
    let (mut world, mut schedule) = setup();
    let [a, b] = riders_with_tails(
        &mut world,
        [
            (Vec2::new(-1.0, 0.0), Vec2::X),
            (Vec2::new(0.0, -1.0), Vec2::Y),
        ],
    );
    schedule.run(&mut world);
    assert!(world.get::<Position>(a).unwrap().0.x < 0.0);
    assert!(world.get::<Position>(b).unwrap().0.y < 0.0);
    // Neither yields: both stay stopped, drain rubber and die together.
    for _ in 0..120 {
        schedule.run(&mut world);
    }
    assert!(!world.get::<IsAlive>(a).unwrap().0);
    assert!(!world.get::<IsAlive>(b).unwrap().0);
}

#[test]
fn equal_arrival_turning_away_survives_and_the_other_rider_goes_on() {
    let (mut world, mut schedule) = setup();
    let [a, b] = riders_with_tails(
        &mut world,
        [
            (Vec2::new(-1.0, 0.0), Vec2::X),
            (Vec2::new(0.0, -1.0), Vec2::Y),
        ],
    );
    schedule.run(&mut world);
    world.get_mut::<ActionState<PlayerInput>>(a).unwrap().0 = PlayerInput::TurnRight;
    schedule.run(&mut world);
    world.get_mut::<ActionState<PlayerInput>>(a).unwrap().0 = PlayerInput::None;
    for _ in 0..120 {
        schedule.run(&mut world);
    }
    assert!(world.get::<IsAlive>(a).unwrap().0, "A could not turn away");
    assert!(world.get::<IsAlive>(b).unwrap().0);
    assert!(world.get::<Position>(b).unwrap().0.y > 100.0);
}

#[test]
fn boosted_head_on_stops_both_short_and_a_turn_still_escapes() {
    let (mut world, mut schedule) = setup();
    let [a, b] = riders_with_tails(
        &mut world,
        [
            (Vec2::new(0.0, 0.0), Vec2::X),
            (Vec2::new(10.0, 0.0), Vec2::NEG_X),
        ],
    );
    for e in [a, b] {
        world.get_mut::<TargetSpeedMult>(e).unwrap().0 = 2.0;
    }
    schedule.run(&mut world);
    let (xa, xb) = (
        world.get::<Position>(a).unwrap().0.x,
        world.get::<Position>(b).unwrap().0.x,
    );
    assert!(xa < 5.0 && xb > 5.0, "riders overlapped: {xa} {xb}");
    world.get_mut::<ActionState<PlayerInput>>(a).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    world.get_mut::<ActionState<PlayerInput>>(a).unwrap().0 = PlayerInput::None;
    for _ in 0..120 {
        schedule.run(&mut world);
    }
    assert!(
        world.get::<IsAlive>(a).unwrap().0,
        "turning away did not escape"
    );
    assert!(!world.get::<IsAlive>(b).unwrap().0);
}

#[test]
fn long_segments_do_not_reach_past_their_corner() {
    let (mut world, mut schedule) = setup();
    let b = cycle(&mut world, Vec2::new(500.0, 300.0), Vec2::Y);
    *world.get_mut::<Trail>(b).unwrap() =
        Trail(vec![Vec2::new(-500.0, 0.0), Vec2::new(500.0, 0.0)]);
    let a = cycle(&mut world, Vec2::new(500.5, -11.0), Vec2::Y);
    for _ in 0..5 {
        schedule.run(&mut world);
        assert!(!world.get::<IsColliding>(a).unwrap().0);
    }
}

#[test]
fn a_rider_stopped_before_a_crossing_lays_no_trail_there() {
    use crate::physics::{Move, resolve_crossings};
    let mv = |x: f32, y: f32, direction: Vec2| Move {
        from: Vec2::new(x, y),
        direction,
        length: 3.0,
    };
    // B stops A at (0,0) a sixth into the tick. A alone would cross (1,0) at
    // half-tick, ahead of C (0.8), and stop C; stopped earlier, it does not.
    let a = mv(-0.5, 0.0, Vec2::X);
    let b = mv(0.0, -0.3, Vec2::Y);
    let c = mv(1.0, -2.4, Vec2::Y);
    let lengths = resolve_crossings(&[a, b, c]);
    assert!(lengths[0].is_some_and(|l| l < 0.5));
    assert_eq!(lengths[1], None);
    assert_eq!(lengths[2], None);
    let without_b = resolve_crossings(&[a, c]);
    assert_eq!(without_b[0], None);
    assert!(without_b[1].is_some_and(|l| l < 2.4));
}
