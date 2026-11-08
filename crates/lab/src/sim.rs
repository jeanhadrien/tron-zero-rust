//! The lab world: the real `simulate_players` system in a bare ECS world, as
//! the server runs it, plus scripted or bot inputs, respawns and an event log.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemState;
use bots::Brain;
use bots::schedule::{SchedulerConfig, Tier, should_redecide};
use bots::world::WorldSnapshotBuilder;
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
    /// Front sensor came within the rubber zone (`SLOW_DOWN_DISTANCE` at least,
    /// wider with speed): the rider closes in exponentially and pays rubber.
    EnterRubberZone {
        at: [f64; 2],
        rubber: f64,
    },
    LeaveRubberZone {
        at: [f64; 2],
        rubber: f64,
    },
    /// A side sensor came within `GRIND_RANGE`: target speed grows.
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
    /// Trail geometry: tail, corners, head.
    pub trail: Vec<Vec2>,
    pub deaths: Vec<Vec2>,
    pub is_bot: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub name: String,
    /// Death count per cause (`wall`, `own_trail`, `trail:<rider>`, ...).
    pub deaths: BTreeMap<String, u32>,
    pub kills: u32,
}

struct LabRider {
    name: String,
    entity: Entity,
    commands: BTreeMap<u32, Action>,
    /// Spawn seed; each placement adds `lives`, like the server's generation.
    seed: u64,
    lives: u64,
    deaths: Vec<Vec2>,
    was_colliding: bool,
    was_sliding: bool,
    bot: Option<LabBot>,
    /// Identity in the rating pool: the bot build id, or the rider's name.
    rated_id: String,
    respawn_after: Option<u32>,
    respawn_at: Option<u32>,
}

/// A bot rider: the server's brain plus its scheduler phase.
struct LabBot {
    brain: Brain,
    slot: usize,
}

pub struct Lab {
    world: World,
    schedule: Schedule,
    snapshot: SystemState<WorldSnapshotBuilder<'static, 'static>>,
    scheduler: SchedulerConfig,
    riders: Vec<LabRider>,
    size: ArenaSize,
    walls: Vec<[f32; 4]>,
    tick: u32,
    events: Vec<Event>,
    eliminations: Vec<Elimination<String>>,
}

impl Lab {
    pub fn new(scenario: &Scenario) -> Self {
        Self::with_seed(scenario, scenario.seed)
    }

    /// Like `new`, with `seed` in place of the scenario's own.
    pub fn with_seed(scenario: &Scenario, seed: u64) -> Self {
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
        world.insert_resource(DeathLog::default());
        world.spawn((Arena, size, WallSegments(walls.clone())));
        let mut schedule = Schedule::default();
        schedule.add_systems(simulate_players);

        let mut riders: Vec<LabRider> = Vec::with_capacity(scenario.riders.len());
        let mut placed: Vec<Vec2> = Vec::new();
        let mut slots = 0;
        for spec in &scenario.riders {
            // Seed 0 keeps the plain name hash, so fixed-start goldens predate
            // seeding unchanged.
            let rider_seed =
                fnv1a(&spec.name).wrapping_add(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut lives = 0;
            let (position, direction) = match (spec.at, spec.heading) {
                (Some(at), Some(heading)) => (Vec2::from(at), heading.vector()),
                _ => {
                    let segments: Vec<[Vec2; 2]> = walls
                        .iter()
                        .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
                        .collect();
                    let spawn = find_safe_spawn(size, &segments, &placed, rider_seed)
                        .unwrap_or_else(|| panic!("{}: no safe spawn", spec.name));
                    lives = 1;
                    (spawn.position, spawn.direction)
                }
            };
            placed.push(position);
            let entity = world
                .spawn((
                    Player,
                    Position(position),
                    Direction(direction),
                    SpeedMult(spec.speed),
                    TargetSpeedMult(spec.speed),
                    Rubber(spec.rubber),
                    IsAlive(true),
                    ShouldHandleDeath(true),
                    Trail::new(position),
                ))
                .id();
            let bot = spec
                .build()
                .expect("validated by Scenario::parse")
                .map(|build| {
                    slots += 1;
                    LabBot {
                        brain: Brain::new(build),
                        slot: slots - 1,
                    }
                });
            riders.push(LabRider {
                name: spec.name.clone(),
                entity,
                commands: spec.commands().expect("validated by Scenario::parse"),
                seed: rider_seed,
                lives,
                deaths: Vec::new(),
                was_colliding: false,
                was_sliding: false,
                rated_id: bot
                    .as_ref()
                    .map_or_else(|| spec.name.clone(), |b| b.brain.build.rating_id()),
                bot,
                respawn_after: spec.respawn_after,
                respawn_at: None,
            });
        }

        let snapshot = SystemState::new(&mut world);
        Self {
            world,
            schedule,
            snapshot,
            scheduler: SchedulerConfig::default(),
            riders,
            size,
            walls,
            tick: 0,
            events: Vec::new(),
            eliminations: Vec::new(),
        }
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// Rated eliminations so far, in tick order (see `shared::Elimination`).
    pub fn eliminations(&self) -> &[Elimination<String>] {
        &self.eliminations
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
            trail: w
                .get::<Trail>(e)
                .unwrap()
                .points(w.get::<Position>(e).unwrap().0),
            deaths: rider.deaths.clone(),
            is_bot: rider.bot.is_some(),
        }
    }

    /// Each rider's deaths by cause, and kills: other riders' deaths on its
    /// trail. Rider order matches `riders()`.
    pub fn scores(&self) -> Vec<Score> {
        let mut scores: Vec<Score> = self
            .riders
            .iter()
            .map(|r| Score {
                name: r.name.clone(),
                ..Score::default()
            })
            .collect();
        for event in &self.events {
            let EventKind::Death { cause, .. } = &event.kind else {
                continue;
            };
            if let Some(score) = scores.iter_mut().find(|s| s.name == event.rider) {
                *score.deaths.entry(cause.clone()).or_default() += 1;
            }
            let killer = cause.strip_prefix("trail:");
            if let Some(score) = scores.iter_mut().find(|s| Some(s.name.as_str()) == killer) {
                score.kills += 1;
            }
        }
        scores
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
            let trail = shared::polyline::length(&view.trail);
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

    /// One server tick, in the server's order: respawns, then bot decisions
    /// and scripted inputs, then `simulate_players`.
    pub fn step(&mut self) {
        let tick = self.tick;
        for index in 0..self.riders.len() {
            let rider = &self.riders[index];
            let scripted = rider.commands.get(&tick) == Some(&Action::Respawn);
            if scripted || rider.respawn_at == Some(tick) {
                self.riders[index].respawn_at = None;
                self.respawn(index);
            }
        }

        let decided = self.decide_bots();
        let before = self.riders();
        for (rider, decided) in self.riders.iter().zip(decided) {
            // Inputs are one-shot: a tick without a command must hold, or a
            // lingering turn would spin the rider.
            let input = match (decided, rider.commands.get(&tick)) {
                (Some(input), _) => input,
                (None, Some(Action::Left)) => PlayerInput::TurnLeft,
                (None, Some(Action::Right)) => PlayerInput::TurnRight,
                _ => PlayerInput::None,
            };
            self.world
                .get_mut::<ActionState<PlayerInput>>(rider.entity)
                .unwrap()
                .0 = input;
        }

        self.schedule.run(&mut self.world);
        // The sim reports what each dying rider was draining against.
        let deaths = std::mem::take(&mut self.world.resource_mut::<DeathLog>().0);
        self.resolve_eliminations(&deaths);

        for (index, before) in before.into_iter().enumerate() {
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
                let rider = &mut self.riders[index];
                rider.deaths.push(before.position);
                rider.was_colliding = false;
                rider.was_sliding = false;
                rider.respawn_at = rider.respawn_after.map(|after| tick + after.max(1));
                let entity = self.riders[index].entity;
                let cause = deaths
                    .iter()
                    .find(|d| d.victim == entity)
                    .map_or_else(|| "unknown".into(), |d| self.cause_label(d.cause));
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

    /// Mirror of the server's `bot_brain_input`: one snapshot per tick in
    /// which any bot redecides, and a turn only on a fresh decision. Returns
    /// each rider's input, `None` for scripted riders.
    fn decide_bots(&mut self) -> Vec<Option<PlayerInput>> {
        let tick = self.tick;
        let mut inputs: Vec<Option<PlayerInput>> = self
            .riders
            .iter()
            .map(|r| r.bot.as_ref().map(|_| PlayerInput::None))
            .collect();
        let redecide: Vec<usize> = (0..self.riders.len())
            .filter(|&i| {
                let rider = &self.riders[i];
                rider.bot.as_ref().is_some_and(|bot| {
                    self.world.get::<IsAlive>(rider.entity).unwrap().0
                        && should_redecide(
                            &self.scheduler,
                            tick,
                            bot.slot,
                            bot.brain.build.knobs.reaction_ticks,
                        )
                })
            })
            .collect();
        if redecide.is_empty() {
            return inputs;
        }
        let world = self
            .snapshot
            .get(&self.world)
            .expect("snapshot queries are infallible")
            .build(tick);
        let alive = world.riders.iter().filter(|r| r.alive).count();
        let tier = Tier::from_load(alive, &self.scheduler);
        for i in redecide {
            let rider = &mut self.riders[i];
            let Some(index) = world.index_of(rider.entity.to_bits()) else {
                continue;
            };
            let bot = rider.bot.as_mut().expect("filtered to bots");
            inputs[i] = Some(bot.brain.think(&world, index, tier).action.to_input());
        }
        inputs
    }

    fn push(&mut self, index: usize, kind: EventKind) {
        self.events.push(Event {
            tick: self.tick,
            rider: self.riders[index].name.clone(),
            kind,
        });
    }

    /// Turn this tick's deaths into rated eliminations, with the riders still
    /// alive at the end of the tick as the suicide field.
    fn resolve_eliminations(&mut self, deaths: &[DeathRecord]) {
        if deaths.is_empty() {
            return;
        }
        let rated_id = |entity: Entity| {
            self.riders
                .iter()
                .find(|r| r.entity == entity)
                .map(|r| r.rated_id.clone())
        };
        let alive: Vec<String> = self
            .riders
            .iter()
            .filter(|r| self.world.get::<IsAlive>(r.entity).is_some_and(|a| a.0))
            .map(|r| r.rated_id.clone())
            .collect();
        let resolved = resolve_eliminations(deaths, rated_id, &alive);
        self.eliminations.extend(resolved);
    }

    /// Trace label for a death cause reported by the sim.
    fn cause_label(&self, cause: DeathCause) -> String {
        match cause {
            DeathCause::Wall => "wall".into(),
            DeathCause::OwnTrail => "own_trail".into(),
            DeathCause::Trail(owner) => {
                let name = self
                    .riders
                    .iter()
                    .find(|r| r.entity == owner)
                    .map_or("?", |r| r.name.as_str());
                format!("trail:{name}")
            }
        }
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
                let head = self.world.get::<Position>(other.entity).unwrap().0;
                riders.push(head);
                let trail = self.world.get::<Trail>(other.entity).unwrap().points(head);
                segments.extend(trail.windows(2).map(|p| [p[0], p[1]]));
            }
        }
        // The server seeds with entity bits + life; the lab uses a stable
        // name hash (plus the scenario seed) so runs replay identically.
        let rider = &mut self.riders[index];
        let seed = rider.seed.wrapping_add(rider.lives);
        rider.lives += 1;
        // The server respawns a bot as a fresh entity with a fresh brain.
        if let Some(bot) = &mut rider.bot {
            bot.brain = Brain::new(bot.brain.build.clone());
        }
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
