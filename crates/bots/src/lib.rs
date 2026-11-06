//! Bot brain: sense → decide over a read-only `WorldSnapshot`.
//!
//! Pure and deterministic (no networking, wall clock or `rand`), so the server
//! drives it from its ECS loop and the lab replays it headless. Bots act only
//! through ordinary `PlayerInput`s; physics is never touched.

pub mod config;
pub mod forecast;
pub mod memory;
pub mod perceive;
pub mod schedule;
pub mod strategy;
pub mod territory;
pub mod world;

use config::{BotBuild, BotKnobs};
use forecast::PhysicsForecaster;
use memory::BotMemory;
use perceive::{Perception, RaySensor, Sensor};
use schedule::Tier;
use strategy::{Action, decide, strategy_for};
use world::WorldSnapshot;

/// One bot's decision state across epochs. A fresh life gets a fresh brain.
#[derive(Clone, Debug)]
pub struct Brain {
    pub build: BotBuild,
    pub memory: BotMemory,
    /// Last decision, kept for observability; never re-emitted as a turn.
    pub cached: Action,
    pub epoch: u32,
    pub degradation: Tier,
}

/// What one epoch decided, with the exact inputs `decide` saw so callers can
/// re-score them for debugging.
pub struct Decision {
    pub action: Action,
    pub perception: Perception,
    /// Build knobs after the LOD tier clamp.
    pub knobs: BotKnobs,
}

impl Brain {
    pub fn new(build: BotBuild) -> Self {
        Self {
            build,
            memory: BotMemory::default(),
            cached: Action::Hold,
            epoch: 0,
            degradation: Tier::Full,
        }
    }

    /// Run one decision epoch for the rider at `index` in `world`: observe
    /// opponents, sense, update memory, then let the build's strategy decide
    /// under the `tier` level of detail.
    pub fn think(&mut self, world: &WorldSnapshot, index: usize, tier: Tier) -> Decision {
        self.degradation = tier;
        let self_id = world.riders[index].id;
        for rider in &world.riders {
            if rider.id != self_id && rider.alive {
                self.memory.opponent.observe(rider.id, rider.dir);
            }
        }
        let sensor = RaySensor {
            rider: index,
            bfs_budget: self.build.knobs.bfs_budget.min(tier.max_bfs()),
        };
        let mut perception = sensor.sense(world);
        self.memory.observe(perception.freedom, perception.exits);
        perception.memory_trend = self.memory.trend();
        perception.opponent_turn_prob = self.memory.opponent.turn_prob();
        let strategy = strategy_for(self.build.personality);
        // Clamp the look-ahead horizon by the LOD tier before strategies see it.
        let mut knobs = self.build.knobs.clone();
        knobs.lookahead_ticks = knobs.lookahead_ticks.min(tier.max_lookahead());
        let action = decide(strategy, &perception, world, &PhysicsForecaster, &knobs);
        self.cached = action;
        self.epoch = self.epoch.wrapping_add(1);
        Decision {
            action,
            perception,
            knobs,
        }
    }
}
