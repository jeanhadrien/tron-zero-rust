//! A bench measures one candidate build against a fixed pool of reference
//! builds: the shared ground of `lab tune` and `lab ladder`.
//!
//! The candidate always plays. For every setup and seed it sits in each seat
//! in turn and the other seats are filled with a window of the pool that
//! moves with the seed and the seat, so every pool build meets it. Results
//! are the tournament's metric rows, folded in a fixed order: they do not
//! depend on the thread count.

use std::collections::BTreeMap;

use bots::config::BotBuild;
use serde::Deserialize;

use crate::scenario::TierPin;
use crate::tournament::{BuildRow, Game, METRICS, SetupSpec, Suite, check_setups, run_games};

/// A candidate's world: the setups and the opponents.
pub struct Bench {
    setups: Vec<SetupSpec>,
    tier: Option<TierPin>,
    pool: Vec<BotBuild>,
}

impl Bench {
    pub fn new(
        setups: Vec<SetupSpec>,
        tier: Option<TierPin>,
        pool: Vec<BotBuild>,
    ) -> Result<Self, String> {
        check_setups(&setups)?;
        if pool.is_empty() {
            return Err("the reference pool is empty".into());
        }
        Ok(Self { setups, tier, pool })
    }

    pub fn setups(&self) -> &[SetupSpec] {
        &self.setups
    }

    pub fn pool(&self) -> &[BotBuild] {
        &self.pool
    }

    fn suite(&self, builds: Vec<BotBuild>, first: u64, count: u64) -> Suite {
        Suite {
            seeds: count,
            quick_seeds: None,
            first_seed: first,
            roster: None,
            tier: self.tier,
            extra_builds: Vec::new(),
            setups: self.setups.clone(),
            builds,
        }
    }

    /// Play `cand` over seeds `first..first + count` on the current rayon
    /// pool; one row per setup, the candidate's.
    pub fn play(&self, cand: &BotBuild, first: u64, count: u64) -> Result<Vec<BuildRow>, String> {
        if self.pool.iter().any(|b| b.name == cand.name) {
            return Err(format!("{:?} is also in the pool", cand.name));
        }
        let mut builds = vec![cand.clone()];
        builds.extend(self.pool.iter().cloned());
        let suite = self.suite(builds, first, count);
        let p = self.pool.len();
        let mut games = Vec::new();
        for (setup, spec) in self.setups.iter().enumerate() {
            for seed in first..first + count {
                for at in 0..spec.riders {
                    let start = seed as usize * spec.riders + at;
                    let mut opponents = (0..spec.riders - 1).map(|j| 1 + (start + j) % p);
                    let seats = (0..spec.riders)
                        .map(|seat| {
                            if seat == at {
                                0
                            } else {
                                opponents.next().unwrap_or(0)
                            }
                        })
                        .collect();
                    games.push(Game { setup, seed, seats });
                }
            }
        }
        let report = run_games(&suite, &games)?;
        report
            .setups
            .into_iter()
            .map(|setup| {
                setup
                    .rows
                    .into_iter()
                    .find(|row| row.build == cand.name)
                    .ok_or_else(|| format!("{}: the candidate never played", setup.name))
            })
            .collect()
    }

    /// `a` against `b` alone on the first setup's rules and arena (two
    /// riders, both seats) over `count` seeds: rounds won by each. Needs a
    /// last-rider-standing setup.
    pub fn duel(
        &self,
        a: &BotBuild,
        b: &BotBuild,
        first: u64,
        count: u64,
    ) -> Result<[u32; 2], String> {
        let mut setup = self.setups[0].clone();
        if !setup.rules.end.last_standing {
            return Err(format!("{}: a duel needs last_standing rules", setup.name));
        }
        setup.riders = 2;
        let mut suite = self.suite(vec![a.clone(), b.clone()], first, count);
        suite.setups = vec![setup];
        let games: Vec<Game> = (first..first + count)
            .flat_map(|seed| {
                [[0, 1], [1, 0]].map(|seats| Game {
                    setup: 0,
                    seed,
                    seats: seats.to_vec(),
                })
            })
            .collect();
        let report = run_games(&suite, &games)?;
        let wins = |name: &str| {
            report.setups[0]
                .rows
                .iter()
                .find(|row| row.build == name)
                .map_or(0, |row| row.wins)
        };
        Ok([wins(&a.name), wins(&b.name)])
    }
}

/// What to maximise: a weighted sum of metric names (see
/// `tournament::METRICS`). Penalties are negative weights. A setup's score is
/// the sum over metrics; the fitness is the mean over setups.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct Fitness(pub BTreeMap<String, f64>);

impl Fitness {
    pub fn validate(&self) -> Result<(), String> {
        if self.0.is_empty() {
            return Err("fitness has no metric".into());
        }
        match self.0.keys().find(|k| !METRICS.contains(&k.as_str())) {
            Some(name) => Err(format!(
                "fitness: unknown metric {name:?}, known: {METRICS:?}"
            )),
            None => Ok(()),
        }
    }

    /// The fitness of one candidate's rows (one per setup). A metric the
    /// setup cannot measure (`win_rate` without last-standing rules) is an
    /// error, not a zero.
    pub fn score(&self, rows: &[BuildRow], setups: &[SetupSpec]) -> Result<f64, String> {
        let mut total = 0.0;
        for (row, setup) in rows.iter().zip(setups) {
            for (name, weight) in &self.0 {
                let value = row.metric(name).ok_or_else(|| {
                    format!("{}: metric {name:?} is undefined in this setup", setup.name)
                })?;
                total += weight * value;
            }
        }
        Ok(total / rows.len() as f64)
    }
}

/// Every metric averaged over the setups (`None` when undefined in any).
pub fn mean_metrics(rows: &[BuildRow]) -> BTreeMap<&'static str, Option<f64>> {
    METRICS
        .iter()
        .map(|&name| {
            let values: Option<Vec<f64>> = rows.iter().map(|r| r.metric(name)).collect();
            (name, values.map(|v| v.iter().sum::<f64>() / v.len() as f64))
        })
        .collect()
}
