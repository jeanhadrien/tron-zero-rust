//! Rollback determinism: restoring the predicted components and replaying the
//! same inputs must reproduce the original run bit for bit. If the sim ever
//! reads state that `protocol.rs` does not `predict()` (a `Local`, a resource,
//! an unregistered component), client rollback diverges and this test fails.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use bevy_ecs::prelude::*;
use glam::Vec2;
use tron_zero_shared::*;

/// Mirrors the `.predict()` registrations in `protocol.rs`. Keep in sync.
type Predicted = (
    Position,
    Direction,
    SpeedMult,
    TargetSpeedMult,
    Rubber,
    IsAlive,
    IsSliding,
    IsColliding,
    ShouldHandleDeath,
    LifeGeneration,
    Trail,
);

const LIFE: LifeGeneration = LifeGeneration(7);

fn snapshot(world: &World, riders: &[Entity]) -> Vec<Predicted> {
    riders
        .iter()
        .map(|&e| {
            (
                *world.get::<Position>(e).unwrap(),
                *world.get::<Direction>(e).unwrap(),
                *world.get::<SpeedMult>(e).unwrap(),
                *world.get::<TargetSpeedMult>(e).unwrap(),
                *world.get::<Rubber>(e).unwrap(),
                *world.get::<IsAlive>(e).unwrap(),
                *world.get::<IsSliding>(e).unwrap(),
                *world.get::<IsColliding>(e).unwrap(),
                *world.get::<ShouldHandleDeath>(e).unwrap(),
                *world.get::<LifeGeneration>(e).unwrap(),
                world.get::<Trail>(e).unwrap().clone(),
            )
        })
        .collect()
}

/// `Debug` prints the shortest round-tripping float, so equal strings mean
/// bitwise-equal state (including the sign of zero).
fn fingerprint(state: &[Predicted]) -> String {
    format!("{state:?}")
}

/// Deterministic turn script: rider `i` turns every `37 + 11 i` ticks,
/// alternating sides, tagged with its life like a human input.
fn input(rider: usize, tick: usize) -> PlayerInput {
    let period = 37 + 11 * rider;
    match (tick % period == period - 1, (tick / period) % 2) {
        (true, 0) => PlayerInput::TurnLeftFor(LIFE.0),
        (true, _) => PlayerInput::TurnRightFor(LIFE.0),
        _ => PlayerInput::None,
    }
}

fn step(world: &mut World, schedule: &mut Schedule, riders: &[Entity], tick: usize) {
    for (i, &e) in riders.iter().enumerate() {
        world.get_mut::<ActionState<PlayerInput>>(e).unwrap().0 = input(i, tick);
    }
    schedule.run(world);
}

#[test]
fn replaying_from_a_predicted_snapshot_is_bit_identical() {
    let mut world = World::new();
    world.insert_resource(SimulationRole::Server);
    world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
    let mut schedule = Schedule::default();
    schedule.add_systems(simulate_players);

    // Three riders that grind, turn and cross trails; the last one runs into
    // the east wall inside the replay window so death cleanup is replayed too.
    let starts = [
        (Vec2::new(-600.0, 0.0), Vec2::X),
        (Vec2::new(-560.0, -300.0), Vec2::Y),
        (Vec2::new(1000.0, 400.0), Vec2::X),
    ];
    let riders: Vec<Entity> = starts
        .iter()
        .map(|&(position, direction)| {
            world
                .spawn((
                    Player,
                    Position(position),
                    Direction(direction),
                    IsAlive(true),
                    ShouldHandleDeath(true),
                    LIFE,
                    Trail::new(position),
                ))
                .id()
        })
        .collect();

    const WARMUP: usize = 60;
    const REPLAY: usize = 360;
    for tick in 0..WARMUP {
        step(&mut world, &mut schedule, &riders, tick);
    }
    let saved = snapshot(&world, &riders);
    let original: Vec<String> = (WARMUP..WARMUP + REPLAY)
        .map(|tick| {
            step(&mut world, &mut schedule, &riders, tick);
            fingerprint(&snapshot(&world, &riders))
        })
        .collect();
    assert!(
        !world.get::<IsAlive>(riders[2]).unwrap().0,
        "scenario drifted: the east-bound rider should die during replay"
    );

    for (&e, state) in riders.iter().zip(saved) {
        world.entity_mut(e).insert(state);
    }
    for (offset, expected) in original.iter().enumerate() {
        step(&mut world, &mut schedule, &riders, WARMUP + offset);
        assert_eq!(
            &fingerprint(&snapshot(&world, &riders)),
            expected,
            "replay diverged at tick {}",
            WARMUP + offset
        );
    }
}
