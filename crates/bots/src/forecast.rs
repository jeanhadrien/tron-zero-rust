//! Rollout forecasting. `PhysicsForecaster` reuses the exact shared physics
//! step so a projected world cannot diverge from the authoritative sim.

use shared::Trail;
use shared::physics::{RiderState, RiderStep, step_riders};

use crate::strategy::Action;
use crate::world::WorldSnapshot;

pub trait Forecaster {
    fn project(&self, world: &WorldSnapshot, actions: &[Action]) -> WorldSnapshot;
}

pub struct PhysicsForecaster;

impl Forecaster for PhysicsForecaster {
    fn project(&self, world: &WorldSnapshot, actions: &[Action]) -> WorldSnapshot {
        // Obstacles are the start-of-tick snapshot, matching `simulate_players`,
        // and the tick is the same `step_riders` transaction, crossings included.
        let obstacles = world.obstacles();
        let mut riders = world.riders.clone();
        let live: Vec<usize> = (0..riders.len()).filter(|&i| riders[i].alive).collect();
        let mut steps: Vec<RiderStep> = live
            .iter()
            .map(|&index| {
                let rider = &mut riders[index];
                RiderStep {
                    owner: Some(index as u64),
                    state: RiderState {
                        position: rider.pos,
                        direction: rider.dir,
                        target_speed: rider.target_speed_mult,
                        rubber: rider.rubber,
                    },
                    turn: actions
                        .get(index)
                        .copied()
                        .unwrap_or(Action::Hold)
                        .to_input(),
                    trail: Trail(std::mem::take(&mut rider.trail)),
                }
            })
            .collect();
        let outcomes = step_riders(&mut steps, obstacles);
        for ((index, step), outcome) in live.into_iter().zip(steps).zip(outcomes) {
            let rider = &mut riders[index];
            rider.trail = step.trail.0;
            rider.pos = outcome.position;
            rider.dir = outcome.direction;
            rider.rubber = outcome.rubber;
            rider.speed_mult = outcome.speed;
            rider.target_speed_mult = outcome.target_speed;
            if outcome.died {
                rider.alive = false;
                rider.trail.clear();
                rider.speed_mult = 0.0;
                rider.target_speed_mult = 0.0;
            }
        }
        WorldSnapshot::projected(world.walls.clone(), riders, world.tick.wrapping_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::RiderSnapshot;
    use bevy_ecs::prelude::*;
    use glam::Vec2;
    use shared::{
        ARENA_HEIGHT, ARENA_WIDTH, ActionState, Arena, Direction, IsAlive, Player, Position,
        Rubber, SimulationRole, SpeedMult, TargetSpeedMult, Trail, WallSegments, arena_walls,
        simulate_players,
    };

    fn cycle(world: &mut World, position: Vec2, direction: Vec2) -> Entity {
        world
            .spawn((
                Player,
                Position(position),
                Direction(direction),
                SpeedMult::base(),
                IsAlive(true),
                shared::ShouldHandleDeath(true),
                Trail::new(position),
            ))
            .id()
    }

    fn rider_snapshot(world: &World, entities: &[Entity]) -> WorldSnapshot {
        let riders = entities
            .iter()
            .map(|entity| {
                let trail = world.get::<Trail>(*entity).unwrap().0.clone();
                RiderSnapshot {
                    id: entity.to_bits(),
                    pos: world.get::<Position>(*entity).unwrap().0,
                    dir: world.get::<Direction>(*entity).unwrap().0,
                    trail,
                    alive: world.get::<IsAlive>(*entity).unwrap().0,
                    rubber: world.get::<Rubber>(*entity).unwrap().0,
                    speed_mult: world.get::<SpeedMult>(*entity).unwrap().0,
                    target_speed_mult: world.get::<TargetSpeedMult>(*entity).unwrap().0,
                }
            })
            .collect();
        WorldSnapshot::new(
            std::sync::Arc::new(arena_walls(ARENA_WIDTH, ARENA_HEIGHT)),
            riders,
            0,
        )
    }

    fn sim_state(world: &World, entity: Entity) -> (Vec2, Vec2, f32) {
        (
            world.get::<Position>(entity).unwrap().0,
            world.get::<Direction>(entity).unwrap().0,
            world.get::<Rubber>(entity).unwrap().0,
        )
    }

    fn forecast_state(snapshot: &WorldSnapshot, entity: Entity) -> (Vec2, Vec2, f32) {
        let rider = snapshot
            .riders
            .iter()
            .find(|r| r.id == entity.to_bits())
            .unwrap();
        (rider.pos, rider.dir, rider.rubber)
    }

    #[test]
    fn forecaster_matches_simulation_trace() {
        let mut world = World::new();
        world.insert_resource(SimulationRole::Server);
        world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
        let a = cycle(&mut world, Vec2::new(-300.0, 0.0), Vec2::X);
        let b = cycle(&mut world, Vec2::new(300.0, 400.0), Vec2::NEG_Y);
        // A cornered trail so turning and side-sliding both come into play.
        *world.get_mut::<Trail>(b).unwrap() = Trail(vec![
            Vec2::new(300.0, 500.0),
            Vec2::new(300.0, 400.0),
            Vec2::new(200.0, 400.0),
        ]);

        let mut schedule = Schedule::default();
        schedule.add_systems(simulate_players);

        let inputs = [
            [Action::Hold, Action::Hold],
            [Action::Hold, Action::Left],
            [Action::Left, Action::Hold],
            [Action::Hold, Action::Right],
            [Action::Right, Action::Hold],
            [Action::Left, Action::Left],
            [Action::Hold, Action::Hold],
        ];

        let mut snapshot = rider_snapshot(&world, &[a, b]);

        for step in &inputs {
            world
                .get_mut::<ActionState<shared::PlayerInput>>(a)
                .unwrap()
                .0 = step[0].to_input();
            world
                .get_mut::<ActionState<shared::PlayerInput>>(b)
                .unwrap()
                .0 = step[1].to_input();
            schedule.run(&mut world);
            snapshot = PhysicsForecaster.project(&snapshot, step);

            for entity in [a, b] {
                let sim = sim_state(&world, entity);
                let forecast = forecast_state(&snapshot, entity);
                assert!(
                    (sim.0 - forecast.0).length() < 1e-3,
                    "position diverged: {:?} vs {:?}",
                    sim.0,
                    forecast.0
                );
                assert!(
                    (sim.1 - forecast.1).length() < 1e-6,
                    "direction diverged: {:?} vs {:?}",
                    sim.1,
                    forecast.1
                );
                assert!(
                    (sim.2 - forecast.2).abs() < 1e-3,
                    "rubber diverged: {} vs {}",
                    sim.2,
                    forecast.2
                );
            }
        }
    }

    #[test]
    fn forecaster_resolves_same_tick_crossings_like_the_simulation() {
        let mut world = World::new();
        world.insert_resource(SimulationRole::Server);
        world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
        // A reaches (0,0) a third into the first tick, B two thirds in: the
        // sim stops B, and the forecast must too.
        let a = cycle(&mut world, Vec2::new(-1.0, 0.0), Vec2::X);
        let b = cycle(&mut world, Vec2::new(0.0, -2.0), Vec2::Y);
        *world.get_mut::<Trail>(a).unwrap() =
            Trail(vec![Vec2::new(-50.0, 0.0), Vec2::new(-1.0, 0.0)]);
        *world.get_mut::<Trail>(b).unwrap() =
            Trail(vec![Vec2::new(0.0, -50.0), Vec2::new(0.0, -2.0)]);
        let mut schedule = Schedule::default();
        schedule.add_systems(simulate_players);

        let mut snapshot = rider_snapshot(&world, &[a, b]);
        let hold = [Action::Hold, Action::Hold];
        for _ in 0..5 {
            schedule.run(&mut world);
            snapshot = PhysicsForecaster.project(&snapshot, &hold);
            for entity in [a, b] {
                let sim = sim_state(&world, entity);
                let forecast = forecast_state(&snapshot, entity);
                assert_eq!(sim, forecast, "forecast diverged from the sim");
            }
        }
        assert!(
            forecast_state(&snapshot, b).0.y < 0.0,
            "B crossed A's trail"
        );
    }
}
