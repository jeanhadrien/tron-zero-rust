//! The lab world: the real `simulate_players` system in a bare ECS world, as
//! the server runs it, plus scripted inputs, respawns and an event log.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use glam::Vec2;
use serde::Serialize;
use shared::spawn::find_safe_spawn;
use shared::*;

use crate::scenario::{Action, Heading, Scenario};

/// Floats in traces are rounded so goldens survive last-bit platform noise.
fn round(x: f32) -> f64 {
    (x as f64 * 1000.0).round() / 1000.0
}

fn point(p: Vec2) -> [f64; 2] {
    [round(p.x), round(p.y)]
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    pub tick: u32,
    pub rider: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    Turn {
        at: [f64; 2],
        heading: &'static str,
    },
    /// Front sensor came within `SLOW_DOWN_DISTANCE`: rubber starts draining.
    EnterRubberZone {
        at: [f64; 2],
        rubber: f64,
    },
    LeaveRubberZone {
        at: [f64; 2],
        rubber: f64,
    },
    /// A side sensor came within `SLOW_DOWN_DISTANCE`: target speed grows.
    SlideStart {
        at: [f64; 2],
        target: f64,
    },
    SlideEnd {
        at: [f64; 2],
        target: f64,
    },
    /// `cause` is what the front sensor faced at the start of the fatal tick:
    /// `wall`, `own_trail`, `trail:<rider>` or `unknown`.
    Death {
        at: [f64; 2],
        cause: String,
    },
    Respawn {
        at: [f64; 2],
        heading: &'static str,
    },
    RespawnFailed {
        reason: &'static str,
    },
    Sample {
        at: [f64; 2],
        heading: &'static str,
        speed: f64,
        target: f64,
        rubber: f64,
        sliding: bool,
        colliding: bool,
        trail: f64,
    },
}

/// Read-only view of one rider, for assertions and rendering.
#[derive(Clone, Debug)]
pub struct RiderView {
    pub name: String,
    pub index: usize,
    pub alive: bool,
    pub position: Vec2,
    pub direction: Vec2,
    pub speed: f32,
    pub target: f32,
    pub rubber: f32,
    pub sliding: bool,
    pub colliding: bool,
    pub trail: Vec<Vec2>,
    pub deaths: Vec<Vec2>,
}

struct LabRider {
    name: String,
    entity: Entity,
    commands: BTreeMap<u32, Action>,
    lives: u64,
    deaths: Vec<Vec2>,
    was_colliding: bool,
    was_sliding: bool,
}

pub struct Lab {
    world: World,
    schedule: Schedule,
    riders: Vec<LabRider>,
    size: ArenaSize,
    walls: Vec<[f32; 4]>,
    tick: u32,
    events: Vec<Event>,
}

impl Lab {
    pub fn new(scenario: &Scenario) -> Self {
        let size = scenario
            .arena
            .map_or_else(ArenaSize::default, |[width, height]| ArenaSize {
                width,
                height,
            });
        let mut walls = arena_walls(size.width, size.height);
        walls.extend(scenario.walls.iter().copied());

        let mut world = World::new();
        world.insert_resource(SimulationRole::Server);
        world.spawn((Arena, size, WallSegments(walls.clone())));
        let mut schedule = Schedule::default();
        schedule.add_systems(simulate_players);

        let riders = scenario
            .riders
            .iter()
            .map(|spec| {
                let position = Vec2::from(spec.at);
                let entity = world
                    .spawn((
                        Player,
                        Position(position),
                        Direction(spec.heading.vector()),
                        SpeedMult(spec.speed),
                        TargetSpeedMult(spec.speed),
                        Rubber(spec.rubber),
                        IsAlive(true),
                        ShouldHandleDeath(true),
                        Trail::new(position),
                    ))
                    .id();
                LabRider {
                    name: spec.name.clone(),
                    entity,
                    commands: spec.commands().expect("validated by Scenario::parse"),
                    lives: 0,
                    deaths: Vec::new(),
                    was_colliding: false,
                    was_sliding: false,
                }
            })
            .collect();

        Self {
            world,
            schedule,
            riders,
            size,
            walls,
            tick: 0,
            events: Vec::new(),
        }
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn size(&self) -> ArenaSize {
        self.size
    }

    /// Arena boundary followed by scenario walls, as `[x1, y1, x2, y2]`.
    pub fn walls(&self) -> &[[f32; 4]] {
        &self.walls
    }

    pub fn riders(&self) -> Vec<RiderView> {
        (0..self.riders.len()).map(|i| self.view(i)).collect()
    }

    pub fn rider(&self, name: &str) -> Option<RiderView> {
        let index = self.riders.iter().position(|r| r.name == name)?;
        Some(self.view(index))
    }

    fn view(&self, index: usize) -> RiderView {
        let rider = &self.riders[index];
        let e = rider.entity;
        let w = &self.world;
        RiderView {
            name: rider.name.clone(),
            index,
            alive: w.get::<IsAlive>(e).unwrap().0,
            position: w.get::<Position>(e).unwrap().0,
            direction: w.get::<Direction>(e).unwrap().0,
            speed: w.get::<SpeedMult>(e).unwrap().0,
            target: w.get::<TargetSpeedMult>(e).unwrap().0,
            rubber: w.get::<Rubber>(e).unwrap().0,
            sliding: w.get::<IsSliding>(e).unwrap().0,
            colliding: w.get::<IsColliding>(e).unwrap().0,
            trail: w.get::<Trail>(e).unwrap().0.clone(),
            deaths: rider.deaths.clone(),
        }
    }

    /// Run until `until` (exclusive), sampling every `sample` ticks if > 0.
    pub fn run(&mut self, until: u32, sample: u32) {
        while self.tick < until {
            if sample > 0 && self.tick.is_multiple_of(sample) {
                self.sample();
            }
            self.step();
        }
    }

    /// Append a `sample` event for every living rider.
    pub fn sample(&mut self) {
        for view in self.riders() {
            if !view.alive {
                continue;
            }
            let trail = Trail(view.trail).length();
            self.push(
                view.index,
                EventKind::Sample {
                    at: point(view.position),
                    heading: Heading::label(view.direction),
                    speed: round(view.speed),
                    target: round(view.target),
                    rubber: round(view.rubber),
                    sliding: view.sliding,
                    colliding: view.colliding,
                    trail: round(trail),
                },
            );
        }
    }

    /// One server tick: respawns, then inputs, then `simulate_players`.
    pub fn step(&mut self) {
        let tick = self.tick;
        for index in 0..self.riders.len() {
            if self.riders[index].commands.get(&tick) == Some(&Action::Respawn) {
                self.respawn(index);
            }
        }

        let before = self.riders();
        let mut causes = Vec::with_capacity(before.len());
        for (rider, view) in self.riders.iter().zip(&before) {
            // Inputs are one-shot: a tick without a command must hold, or a
            // lingering turn would spin the rider.
            let input = match rider.commands.get(&tick) {
                Some(Action::Left) => PlayerInput::TurnLeft,
                Some(Action::Right) => PlayerInput::TurnRight,
                _ => PlayerInput::None,
            };
            self.world
                .get_mut::<ActionState<PlayerInput>>(rider.entity)
                .unwrap()
                .0 = input;
            // Death cleanup clears trails, so the cause must be read now.
            causes.push(view.alive.then(|| self.front_cause(view.index, input)));
        }

        self.schedule.run(&mut self.world);

        for (index, (before, cause)) in before.into_iter().zip(causes).enumerate() {
            let after = self.view(index);
            if !before.alive {
                continue;
            }
            if after.direction != before.direction {
                self.push(
                    index,
                    EventKind::Turn {
                        at: point(before.position),
                        heading: Heading::label(after.direction),
                    },
                );
            }
            if !after.alive {
                self.riders[index].deaths.push(before.position);
                self.riders[index].was_colliding = false;
                self.riders[index].was_sliding = false;
                let cause = cause.flatten().unwrap_or_else(|| "unknown".into());
                self.push(
                    index,
                    EventKind::Death {
                        at: point(before.position),
                        cause,
                    },
                );
                continue;
            }
            let at = point(after.position);
            let rider = &mut self.riders[index];
            let zone = (rider.was_colliding, after.colliding);
            let slide = (rider.was_sliding, after.sliding);
            rider.was_colliding = after.colliding;
            rider.was_sliding = after.sliding;
            let rubber = round(after.rubber);
            let target = round(after.target);
            match zone {
                (false, true) => self.push(index, EventKind::EnterRubberZone { at, rubber }),
                (true, false) => self.push(index, EventKind::LeaveRubberZone { at, rubber }),
                _ => {}
            }
            match slide {
                (false, true) => self.push(index, EventKind::SlideStart { at, target }),
                (true, false) => self.push(index, EventKind::SlideEnd { at, target }),
                _ => {}
            }
        }
        self.tick += 1;
    }

    fn push(&mut self, index: usize, kind: EventKind) {
        self.events.push(Event {
            tick: self.tick,
            rider: self.riders[index].name.clone(),
            kind,
        });
    }

    /// What the front sensor faces this tick, using the same obstacle model
    /// and self-exclusions as the sim (turn applied before sensing).
    fn front_cause(&self, index: usize, input: PlayerInput) -> Option<String> {
        let me = &self.riders[index];
        let position = self.world.get::<Position>(me.entity).unwrap().0;
        let mut heading = self.world.get::<Direction>(me.entity).unwrap().0;
        match input {
            PlayerInput::TurnLeft => heading = rotate_left(heading),
            PlayerInput::TurnRight => heading = rotate_right(heading),
            _ => {}
        }
        let mut groups: Vec<(String, Vec<physics::Segment>)> = vec![(
            "wall".into(),
            self.walls
                .iter()
                .map(|w| physics::Segment {
                    a: Vec2::new(w[0], w[1]),
                    b: Vec2::new(w[2], w[3]),
                    owner: None,
                    from_head: 0,
                })
                .collect(),
        )];
        for (other_index, other) in self.riders.iter().enumerate() {
            if !self.world.get::<IsAlive>(other.entity).unwrap().0 {
                continue;
            }
            let mut trail = self.world.get::<Trail>(other.entity).unwrap().clone();
            if other_index == index && input != PlayerInput::None {
                trail.turn(position);
            }
            let n = trail.0.len();
            let owner = Some(other.entity.to_bits());
            let label = if other_index == index {
                "own_trail".into()
            } else {
                format!("trail:{}", other.name)
            };
            let segments = trail
                .0
                .windows(2)
                .enumerate()
                .map(|(i, p)| physics::Segment {
                    a: p[0],
                    b: p[1],
                    owner,
                    from_head: n - 2 - i,
                })
                .collect();
            groups.push((label, segments));
        }
        groups
            .into_iter()
            .map(|(label, segments)| {
                let distance = physics::nearest(
                    &segments,
                    Some(me.entity.to_bits()),
                    position,
                    heading,
                    false,
                );
                (label, distance)
            })
            .filter(|(_, distance)| distance.is_finite())
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(label, _)| label)
    }

    /// Mirror of the server's `try_respawn` reset: same safe-spawn inputs
    /// (walls + living trails, living riders) and the same component reset.
    fn respawn(&mut self, index: usize) {
        let entity = self.riders[index].entity;
        if self.world.get::<IsAlive>(entity).unwrap().0 {
            self.push(index, EventKind::RespawnFailed { reason: "alive" });
            return;
        }
        let mut segments: Vec<[Vec2; 2]> = self
            .walls
            .iter()
            .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
            .collect();
        let mut riders = Vec::new();
        for other in &self.riders {
            if self.world.get::<IsAlive>(other.entity).unwrap().0 {
                riders.push(self.world.get::<Position>(other.entity).unwrap().0);
                let trail = &self.world.get::<Trail>(other.entity).unwrap().0;
                segments.extend(trail.windows(2).map(|p| [p[0], p[1]]));
            }
        }
        // The server seeds with entity bits + life; the lab uses a stable
        // name hash so scenarios replay identically across runs.
        let rider = &mut self.riders[index];
        let seed = fnv1a(&rider.name).wrapping_add(rider.lives);
        rider.lives += 1;
        let Some(spawn) = find_safe_spawn(self.size, &segments, &riders, seed) else {
            self.push(
                index,
                EventKind::RespawnFailed {
                    reason: "no_safe_space",
                },
            );
            return;
        };
        self.world.entity_mut(entity).insert((
            Position(spawn.position),
            Direction(spawn.direction),
            Velocity(spawn.direction * (BASE_SPEED * TICK_SECS * 1000.0)),
            SpeedMult::base(),
            TargetSpeedMult::default(),
            Rubber::default(),
            IsAlive(true),
            Trail::new(spawn.position),
            ShouldHandleDeath(true),
            IsSliding(false),
            IsColliding(false),
            ActionState(PlayerInput::None),
        ));
        self.push(
            index,
            EventKind::Respawn {
                at: point(spawn.position),
                heading: Heading::label(spawn.direction),
            },
        );
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}
