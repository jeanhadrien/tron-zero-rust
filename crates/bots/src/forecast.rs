//! Rollout forecasting. A [`Rollout`] steps one bot and the riders near it
//! through the exact shared physics (`step_riders`, the sim's own tick), so a
//! projected line cannot disagree with what the sim would do. The bot steers;
//! the others ride straight on.

use glam::Vec2;
use shared::physics::{RiderState, RiderStep, Segment, step_riders};

use crate::plan::Action;
use crate::world::WorldSnapshot;

/// A bot and its neighbours, stepped tick by tick.
pub struct Rollout {
    /// The bot first, then the riders close enough to matter.
    steps: Vec<RiderStep>,
    /// Snapshot index of each step; the owner tag of its trail segments.
    ids: Vec<usize>,
    /// Walls and the trails of riders that are not stepped. Obstacles are
    /// rebuilt every tick as these plus the stepped riders' growing trails,
    /// so the start-of-tick snapshot matches `simulate_players`.
    statics: Vec<Segment>,
    obstacles: Vec<Segment>,
    points: Vec<Vec2>,
}

impl Rollout {
    /// Start from `world` with bot `me` and every living rider within `radius`
    /// of it. `None` when `me` is dead.
    pub fn new(world: &WorldSnapshot, me: usize, radius: f32) -> Option<Self> {
        let bot = world.riders.get(me).filter(|r| r.alive)?;
        let mut ids = vec![me];
        ids.extend(
            world
                .riders
                .iter()
                .enumerate()
                .filter(|&(i, r)| i != me && r.alive && r.pos.distance(bot.pos) <= radius)
                .map(|(i, _)| i),
        );
        let steps = ids
            .iter()
            .map(|&i| {
                let rider = &world.riders[i];
                RiderStep {
                    owner: Some(i as u64),
                    state: RiderState {
                        position: rider.pos,
                        direction: rider.dir,
                        target_speed: rider.target_speed_mult,
                        rubber: rider.rubber,
                    },
                    turn: Action::Hold.to_input(),
                    trail: rider.trail.clone(),
                }
            })
            .collect();
        let statics: Vec<Segment> = world
            .obstacles()
            .iter()
            .filter(|s| s.owner.is_none_or(|owner| !ids.contains(&(owner as usize))))
            .copied()
            .collect();
        Some(Self {
            steps,
            obstacles: statics.clone(),
            statics,
            ids,
            points: Vec::new(),
        })
    }

    /// One tick: the bot does `turn`, the others hold. `false` when the bot
    /// died. A neighbour that dies leaves no trail, as in the sim.
    pub fn step(&mut self, turn: Action) -> bool {
        self.obstacles.truncate(self.statics.len());
        for (step, &id) in self.steps.iter().zip(&self.ids) {
            step.trail
                .points_into(step.state.position, &mut self.points);
            self.obstacles
                .extend(Segment::trail_legs(&self.points, id as u64));
        }
        self.steps[0].turn = turn.to_input();
        let outcomes = step_riders(&mut self.steps, &self.obstacles);
        let mut dead = Vec::new();
        for (index, (step, outcome)) in self.steps.iter_mut().zip(&outcomes).enumerate() {
            if outcome.died {
                if index == 0 {
                    return false;
                }
                dead.push(index);
                continue;
            }
            step.state = RiderState {
                position: outcome.position,
                direction: outcome.direction,
                target_speed: outcome.target_speed,
                rubber: outcome.rubber,
            };
        }
        for index in dead.into_iter().rev() {
            self.steps.remove(index);
            self.ids.remove(index);
        }
        true
    }

    /// The bot's state.
    pub fn me(&self) -> &RiderState {
        &self.steps[0].state
    }

    /// State of the rider at snapshot index `id`, if it is stepped and alive.
    pub fn state_of(&self, id: usize) -> Option<&RiderState> {
        let at = self.ids.iter().position(|&i| i == id)?;
        Some(&self.steps[at].state)
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
                let trail = world.get::<Trail>(*entity).unwrap().clone();
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

    fn rollout_state(state: &RiderState) -> (Vec2, Vec2, f32) {
        (state.position, state.direction, state.rubber)
    }

    #[test]
    fn rollout_matches_simulation_trace() {
        let mut world = World::new();
        world.insert_resource(SimulationRole::Server);
        world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
        let a = cycle(&mut world, Vec2::new(-300.0, 0.0), Vec2::X);
        let b = cycle(&mut world, Vec2::new(300.0, 400.0), Vec2::NEG_Y);
        // A cornered trail so turning and side-sliding both come into play.
        *world.get_mut::<Trail>(b).unwrap() =
            Trail(vec![Vec2::new(200.0, 500.0), Vec2::new(300.0, 500.0)]);

        let mut schedule = Schedule::default();
        schedule.add_systems(simulate_players);

        let inputs = [
            Action::Hold,
            Action::Hold,
            Action::Left,
            Action::Hold,
            Action::Right,
            Action::Left,
            Action::Left,
            Action::Hold,
        ];

        let snapshot = rider_snapshot(&world, &[a, b]);
        let mut rollout = Rollout::new(&snapshot, 0, 5000.0).unwrap();

        for action in inputs {
            world
                .get_mut::<ActionState<shared::PlayerInput>>(a)
                .unwrap()
                .0 = action.to_input();
            schedule.run(&mut world);
            assert!(rollout.step(action));

            let sim = sim_state(&world, a);
            let forecast = rollout_state(rollout.me());
            assert!(
                (sim.0 - forecast.0).length() < 1e-3,
                "position diverged: {:?} vs {:?}",
                sim.0,
                forecast.0
            );
            assert_eq!(sim.1, forecast.1, "direction diverged");
            assert!((sim.2 - forecast.2).abs() < 1e-3, "rubber diverged");
            let sim_b = sim_state(&world, b);
            let forecast_b = rollout_state(rollout.state_of(1).unwrap());
            assert!((sim_b.0 - forecast_b.0).length() < 1e-3, "foe diverged");
        }
    }

    #[test]
    fn rollout_resolves_same_tick_crossings_like_the_simulation() {
        let mut world = World::new();
        world.insert_resource(SimulationRole::Server);
        world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
        // A reaches (0,0) a third into the first tick, B two thirds in: the
        // sim stops B, and the rollout must too.
        let a = cycle(&mut world, Vec2::new(-1.0, 0.0), Vec2::X);
        let b = cycle(&mut world, Vec2::new(0.0, -2.0), Vec2::Y);
        *world.get_mut::<Trail>(a).unwrap() = Trail(vec![Vec2::new(-50.0, 0.0)]);
        *world.get_mut::<Trail>(b).unwrap() = Trail(vec![Vec2::new(0.0, -50.0)]);
        let mut schedule = Schedule::default();
        schedule.add_systems(simulate_players);

        let snapshot = rider_snapshot(&world, &[a, b]);
        let mut rollout = Rollout::new(&snapshot, 0, 5000.0).unwrap();
        for _ in 0..5 {
            schedule.run(&mut world);
            assert!(rollout.step(Action::Hold));
            assert_eq!(
                sim_state(&world, a),
                rollout_state(rollout.me()),
                "rollout diverged from the sim"
            );
            assert_eq!(
                sim_state(&world, b),
                rollout_state(rollout.state_of(1).unwrap()),
                "rollout diverged from the sim"
            );
        }
        assert!(rollout.state_of(1).unwrap().position.y < 0.0, "B crossed A");
    }

    #[test]
    fn a_rider_driving_into_a_wall_dies() {
        let mut world = World::new();
        world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
        let a = cycle(&mut world, Vec2::new(900.0, 0.0), Vec2::X);
        let snapshot = rider_snapshot(&world, &[a]);
        let mut rollout = Rollout::new(&snapshot, 0, 5000.0).unwrap();
        let alive_for = (0..200).take_while(|_| rollout.step(Action::Hold)).count();
        // 100 units at 3 per tick, then the rubber runs out.
        assert!((40..80).contains(&alive_for), "{alive_for}");
    }
}
