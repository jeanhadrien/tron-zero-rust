use super::*;
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

#[test]
fn cardinal_velocity_preserves_360_units_per_second() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    schedule.run(&mut world);
    near(world.get::<Position>(entity).unwrap().0.x, 3.0);
    near(world.get::<Velocity>(entity).unwrap().0.x, 3000.0);
    near(world.get::<Trail>(entity).unwrap().length(), 3.0);
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
    let entity = cycle(&mut world, Vec2::new(1199.0, 0.0), Vec2::X);
    world.get_mut::<Rubber>(entity).unwrap().0 = 4.0;
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<IsAlive>(entity).unwrap().0);
    near(world.get::<Rubber>(entity).unwrap().0, 1.12);
    schedule.run(&mut world);
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
fn escape_regenerates_rubber_and_preserves_speed_inertia() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::new(1199.0, 0.0), Vec2::X);
    world.get_mut::<TargetSpeedMult>(entity).unwrap().0 = 2.0;
    world.get_mut::<Rubber>(entity).unwrap().0 = 70.0;
    schedule.run(&mut world);
    assert!(world.get::<SpeedMult>(entity).unwrap().0 < 1.0);
    near(world.get::<TargetSpeedMult>(entity).unwrap().0, 2.0);
    let rubber = world.get::<Rubber>(entity).unwrap().0;
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    assert!(!world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<Rubber>(entity).unwrap().0 > rubber);
    near(world.get::<SpeedMult>(entity).unwrap().0, 2.0);
    assert!(world.get::<IsSliding>(entity).unwrap().0);
    assert!(world.get::<TargetSpeedMult>(entity).unwrap().0 > 2.0);
}

#[test]
fn slide_boosts_next_tick_and_open_space_decays_gradually() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::new(0.0, 1195.0), Vec2::X);
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
    near(world.get::<TargetSpeedMult>(entity).unwrap().0, 1.9964);
    near(world.get::<SpeedMult>(entity).unwrap().0, 2.0);
}

#[test]
fn opponent_active_trail_can_be_a_sliding_wall() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    let other = cycle(&mut world, Vec2::new(100.0, 5.0), Vec2::Y);
    *world.get_mut::<Trail>(other).unwrap() =
        Trail(vec![Vec2::new(-10.0, 5.0), Vec2::new(100.0, 5.0)]);
    schedule.run(&mut world);
    assert!(world.get::<IsSliding>(entity).unwrap().0);
    assert!(!world.get::<IsColliding>(entity).unwrap().0);
    assert!(world.get::<TargetSpeedMult>(entity).unwrap().0 > 1.0);
}

#[test]
fn exact_boundary_contact_blocks_outward_but_allows_escape_or_tangent() {
    for heading in [Vec2::X, Vec2::NEG_X, Vec2::Y] {
        let (mut world, mut schedule) = setup();
        let entity = cycle(&mut world, Vec2::new(1200.0, 0.0), heading);
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
        *world.get_mut::<Trail>(other).unwrap() = Trail(points);
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
        Vec2::ZERO,
    ]);
    schedule.run(&mut world);
    assert!(world.get::<IsColliding>(entity).unwrap().0);

    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![Vec2::NEG_X * 10.0, Vec2::ZERO]);
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    near(world.get::<Position>(entity).unwrap().0.x, 0.0);
    near(world.get::<Position>(entity).unwrap().0.y, 3.0);
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
    *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![Vec2::NEG_X * 10.0, Vec2::ZERO]);
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
            *world.get_mut::<Trail>(other).unwrap() =
                Trail(vec![Vec2::new(50.0, -10.0), Vec2::new(50.0, 10.0)]);
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
fn straight_and_cornered_trails_consume_oldest_end_gradually() {
    let mut straight = Trail::new(Vec2::ZERO);
    straight.advance(Vec2::X * 100.0, 40.0);
    assert_eq!(straight.0, vec![Vec2::X * 60.0, Vec2::X * 100.0]);
    straight.advance(Vec2::X * 103.0, 40.0);
    assert_eq!(straight.0[0], Vec2::X * 63.0);
    near(straight.length(), 40.0);
    let mut corners = Trail(vec![
        Vec2::ZERO,
        Vec2::new(10.0, 0.0),
        Vec2::new(10.0, 10.0),
        Vec2::new(20.0, 10.0),
        Vec2::new(20.0, 20.0),
    ]);
    corners.advance(Vec2::new(20.0, 25.0), 20.0);
    assert_eq!(
        corners.0,
        vec![
            Vec2::new(15.0, 10.0),
            Vec2::new(20.0, 10.0),
            Vec2::new(20.0, 25.0),
        ]
    );
    near(corners.length(), 20.0);
    corners.advance(Vec2::new(20.0, 60.0), 20.0);
    assert_eq!(
        corners.0,
        vec![Vec2::new(20.0, 40.0), Vec2::new(20.0, 60.0)]
    );
}

#[test]
fn simulation_caps_long_straight_trail_without_erasing_active_segment() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::new(-1000.0, 0.0), Vec2::X);
    for _ in 0..650 {
        schedule.run(&mut world);
    }
    let trail = world.get::<Trail>(entity).unwrap();
    near(trail.length(), TRAIL_MAX_LENGTH);
    assert_eq!(trail.0.len(), 2);
    assert!(trail.0[0].distance(trail.0[1]) > 0.0);
    assert_eq!(trail.0[1], world.get::<Position>(entity).unwrap().0);
    assert!(world.get::<IsAlive>(entity).unwrap().0);
}

#[test]
fn zero_length_turns_do_not_churn_vertices() {
    let mut trail = Trail::new(Vec2::ZERO);
    for _ in 0..10 {
        trail.turn(Vec2::ZERO);
    }
    assert_eq!(trail.0.len(), 2);
    trail.advance(Vec2::X * 3.0, TRAIL_MAX_LENGTH);
    trail.turn(Vec2::X * 3.0);
    trail.advance(Vec2::new(3.0, 3.0), TRAIL_MAX_LENGTH);
    assert_eq!(
        trail.0,
        vec![Vec2::ZERO, Vec2::X * 3.0, Vec2::new(3.0, 3.0)]
    );
}

#[test]
fn dead_opponents_are_not_obstacles() {
    let (mut world, mut schedule) = setup();
    let entity = cycle(&mut world, Vec2::ZERO, Vec2::X);
    let other = cycle(&mut world, Vec2::new(1.0, 10.0), Vec2::Y);
    *world.get_mut::<Trail>(other).unwrap() =
        Trail(vec![Vec2::new(1.0, -10.0), Vec2::new(1.0, 10.0)]);
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
    assert_eq!(world.entities().len(), 3);
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
        HistoryState::Updated(Trail(vec![Vec2::new(5.0, -10.0), Vec2::new(5.0, 10.0)])),
    );
    trails.insert_explicit(
        Tick(20),
        HistoryState::Updated(Trail::new(Vec2::new(100.0, 100.0))),
    );
    let mut lives = ConfirmedHistory::<IsAlive>::default();
    lives.insert_explicit(Tick(10), HistoryState::Updated(IsAlive(true)));
    world.entity_mut(remote).insert((trails, lives));
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
    world.get_mut::<ActionState<PlayerInput>>(entity).unwrap().0 = PlayerInput::TurnLeft;
    schedule.run(&mut world);
    let result = world.get::<Trail>(entity).unwrap().clone();
    world
        .entity_mut(entity)
        .insert((trail, position, direction));
    schedule.run(&mut world);
    assert_eq!(*world.get::<Trail>(entity).unwrap(), result);
    assert_eq!(world.entities().len(), 2);
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
                *world.get_mut::<Trail>(entity).unwrap() = Trail(vec![Vec2::new(5.0, -10.0), p]);
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
