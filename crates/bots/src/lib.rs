//! Bot brain: look at a read-only `WorldSnapshot`, plan a maneuver, follow it.
//!
//! Pure and deterministic (no networking, wall clock or `rand`), so any runner
//! can drive it: `drive::drive` runs one tick for a set of brains. Bots act only
//! through ordinary `PlayerInput`s; physics is never touched.
//!
//! Difficulty is a budget (`config::BotKnobs`: reaction, horizon, variety,
//! noise); personality is the weight vector of the objective
//! (`objective::Weights`). One planner serves every build.

pub mod config;
pub mod drive;
pub mod forecast;
pub mod grind;
pub mod noise;
pub mod objective;
pub mod plan;
pub mod planner;
pub mod race;
pub mod roster;
pub mod schedule;
pub mod space;
pub mod style;
pub mod tuning;
pub mod world;

use config::BotBuild;
use plan::{Action, Plan};
use planner::{Params, Report, plan as plan_maneuver, survives};
use schedule::{SchedulerConfig, Tier, should_redecide};
use space::Scratch;
use world::WorldSnapshot;

/// One bot's decision state across a life (or a round). A fresh life or round
/// gets a fresh brain.
#[derive(Clone, Debug)]
pub struct Brain {
    pub build: BotBuild,
    /// The turns still to emit.
    plan: Plan,
    /// Seeds the bot's misjudgments, so two brains with the same build and the
    /// same view still err differently. The caller picks it.
    seed: u64,
    /// Phase of this bot's planning ticks, so bots do not all plan at once.
    slot: usize,
    scratch: Scratch,
    /// Tick of the first look: the start of this brain's round.
    born: Option<u32>,
    /// The opponent being pressured, kept from one plan to the next so the
    /// bot does not dither between targets.
    target: Option<u64>,
    /// The last plan was to ride straight on.
    steady: bool,
    /// Plays a round of last rider standing: the round heats up.
    round: bool,
}

/// What one tick decided.
pub struct Decision {
    /// The steering input for this tick.
    pub action: Action,
    /// Set when this tick planned anew.
    pub report: Option<Report>,
}

impl Brain {
    pub fn new(build: BotBuild, seed: u64, slot: usize) -> Self {
        Self {
            build,
            plan: Plan::default(),
            seed,
            slot,
            scratch: Scratch::default(),
            born: None,
            target: None,
            steady: false,
            round: false,
        }
    }

    /// The same bot for a new life: no plan, a new seed.
    pub fn reborn(&self, seed: u64) -> Self {
        Self::new(self.build.clone(), seed, self.slot)
    }

    /// The same bot for a new round of last rider standing: it grows bolder
    /// as the round ages, so rounds come to an end.
    pub fn reborn_for_round(&self, seed: u64) -> Self {
        Self {
            round: true,
            ..self.reborn(seed)
        }
    }

    /// Whether this tick needs a look at the world: time to plan again, or a
    /// planned turn is due (and is checked before it is made). Other ticks
    /// are `Action::Hold` without a snapshot.
    pub fn wants_view(&self, tick: u32, scheduler: &SchedulerConfig) -> bool {
        should_redecide(scheduler, tick, self.slot, self.build.knobs.reaction_ticks)
            || self.plan.due(tick)
    }

    /// The input for tick `world.tick` for the rider at `index`, under the
    /// level of detail `tier`. Plans anew on the reaction cadence, or at once
    /// when a due turn no longer survives the physics; otherwise follows the
    /// plan.
    pub fn act(
        &mut self,
        world: &WorldSnapshot,
        index: usize,
        tier: Tier,
        scheduler: &SchedulerConfig,
    ) -> Decision {
        let tick = world.tick;
        let mut params = Params::new(&self.build, tier, self.seed);
        params.steady = self.steady;
        params.rounds = self.round;
        params.age = tick.saturating_sub(*self.born.get_or_insert(tick));
        self.plan.drop_before(tick);
        let cadence = should_redecide(scheduler, tick, self.slot, self.build.knobs.reaction_ticks);
        let rest = self.plan.remaining(tick);
        let replan = cadence
            || rest
                .as_ref()
                .is_some_and(|rest| self.plan.due(tick) && !survives(world, index, rest, &params));
        let report = replan.then(|| {
            let outcome =
                plan_maneuver(world, index, &params, &mut self.scratch, rest, self.target);
            self.target = outcome.report.target;
            self.steady = outcome.maneuver.kind == plan::Kind::Hold;
            self.plan = Plan::start(tick, &outcome.maneuver);
            outcome.report
        });
        Decision {
            action: self.plan.take(tick),
            report,
        }
    }
}
