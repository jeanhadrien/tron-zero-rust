//! `lab tune`: search the knobs and weights of a bot build for a game setup.
//!
//! A tune file (TOML) names a base build, the parameters to search with their
//! ranges, the setups and the reference pool to play against, two seed sets
//! and the fitness (see `bench`):
//!
//! ```toml
//! seed = 1                    # the search's random seed
//! train_seeds = 4             # the search sees seeds train_first.. (default 1)
//! holdout_seeds = 8           # the winner is re-scored on holdout_first.. (default 100000)
//! default_roster = true       # the pool: default roster, a roster file, [[pool]] builds
//!
//! [base]                      # the build to start from, see `crate::builds`
//! name = "Tuned"
//! bot = "trapper"
//!
//! [[param]]                   # any knob or weight, by bare name
//! name = "pressure"
//! min = 0.0
//! max = 4.0
//!
//! [fitness]                   # weighted sum of tournament metrics
//! win_rate = 1.0
//! idle_share = -0.5
//!
//! [[setup]]                   # as in a suite
//! name = "rounds4"
//! riders = 4
//! [setup.match]
//! rounds = 5
//! respawn = "never"
//! [setup.match.end]
//! last_standing = true
//! time_limit = 3600
//!
//! [sweep]                     # optional
//! steps = 5                   # values per parameter
//! [search]                    # optional
//! population = 8              # default: CMA-ES's own choice
//! generations = 10
//! sigma = 0.25                # initial step, as a share of each range
//! ```
//!
//! Two modes: `sweep` moves one parameter at a time and prints how every
//! metric follows (a parameter that moves nothing is a dead knob); `search`
//! runs CMA-ES over all parameters at once. Fitness is deterministic for a
//! given build and seed set, so the search is reproducible from `seed` and
//! does not depend on the thread count.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bots::config::BotBuild;
use cmaes::{CMAESOptions, DVector, Mode};
use serde::{Deserialize, Serialize};

use crate::bench::{Bench, Fitness, mean_metrics};
use crate::builds::{self, BuildSpec};
use crate::scenario::TierPin;
use crate::tournament::{METRICS, SetupSpec, thread_pool};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default = "one")]
    seed: u64,
    train_seeds: u64,
    #[serde(default = "one")]
    train_first: u64,
    holdout_seeds: u64,
    #[serde(default = "holdout_first")]
    holdout_first: u64,
    #[serde(default)]
    tier: Option<TierPin>,
    base: BuildSpec,
    #[serde(rename = "param")]
    params: Vec<ParamSpec>,
    #[serde(default)]
    default_roster: bool,
    #[serde(default)]
    roster: Option<PathBuf>,
    #[serde(default, rename = "pool")]
    pool_builds: Vec<BuildSpec>,
    #[serde(rename = "setup")]
    setups: Vec<SetupSpec>,
    fitness: Fitness,
    #[serde(default)]
    sweep: SweepSpec,
    #[serde(default)]
    search: SearchSpec,
}

fn one() -> u64 {
    1
}

fn holdout_first() -> u64 {
    100_000
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamSpec {
    name: String,
    min: f64,
    max: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SweepSpec {
    #[serde(default = "five")]
    steps: usize,
}

impl Default for SweepSpec {
    fn default() -> Self {
        Self { steps: 5 }
    }
}

fn five() -> usize {
    5
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchSpec {
    #[serde(default)]
    population: Option<usize>,
    #[serde(default = "twenty")]
    generations: usize,
    #[serde(default = "quarter")]
    sigma: f64,
}

impl Default for SearchSpec {
    fn default() -> Self {
        Self {
            population: None,
            generations: 20,
            sigma: 0.25,
        }
    }
}

fn twenty() -> usize {
    20
}

fn quarter() -> f64 {
    0.25
}

/// One searched parameter, resolved against the base build.
#[derive(Clone, Debug)]
struct Dim {
    name: String,
    min: f64,
    max: f64,
    integer: bool,
    base: f64,
}

/// A loaded tune file.
pub struct Tune {
    seed: u64,
    train: (u64, u64),
    holdout: (u64, u64),
    base: BotBuild,
    dims: Vec<Dim>,
    bench: Bench,
    fitness: Fitness,
    sweep: SweepSpec,
    search: SearchSpec,
}

/// A candidate's score on one seed set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Eval {
    pub fitness: f64,
    /// Every metric of `tournament::METRICS`, averaged over the setups.
    pub metrics: BTreeMap<&'static str, Option<f64>>,
}

impl Tune {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(""));
        Self::parse(&text, base).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str, dir: &Path) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
        file.fitness.validate()?;
        let base = file.base.build()?;
        let known = builds::params(&base)?;
        let mut dims: Vec<Dim> = Vec::new();
        for spec in &file.params {
            let param = known.get(&spec.name).ok_or_else(|| {
                format!(
                    "param {:?}: not a knob or weight of the base build, known: {:?}",
                    spec.name,
                    known.keys().collect::<Vec<_>>()
                )
            })?;
            if spec.max <= spec.min {
                return Err(format!("param {:?}: max must exceed min", spec.name));
            }
            if dims.iter().any(|d| d.name == spec.name) {
                return Err(format!("param {:?} listed twice", spec.name));
            }
            dims.push(Dim {
                name: spec.name.clone(),
                min: spec.min,
                max: spec.max,
                integer: param.integer,
                base: param.value,
            });
        }
        if dims.is_empty() {
            return Err("give at least one [[param]]".into());
        }
        if file.train_seeds == 0 || file.holdout_seeds == 0 {
            return Err("train_seeds and holdout_seeds must be at least 1".into());
        }
        let train_end = file.train_first + file.train_seeds;
        let holdout_end = file.holdout_first + file.holdout_seeds;
        if file.train_first < holdout_end && file.holdout_first < train_end {
            return Err("the train and holdout seed sets overlap".into());
        }
        if file.search.sigma <= 0.0 || file.search.generations == 0 || file.sweep.steps < 2 {
            return Err("search.sigma > 0, search.generations >= 1 and sweep.steps >= 2".into());
        }
        let pool = builds::resolve(
            file.default_roster,
            file.roster.as_deref(),
            &file.pool_builds,
            dir,
        )?;
        let bench = Bench::new(file.setups, file.tier, pool)?;
        Ok(Self {
            seed: file.seed,
            train: (file.train_first, file.train_seeds),
            holdout: (file.holdout_first, file.holdout_seeds),
            base,
            dims,
            bench,
            fitness: file.fitness,
            sweep: file.sweep,
            search: file.search,
        })
    }

    /// The parameter values a point of the unit cube stands for.
    fn decode(&self, x: &[f64]) -> Vec<(String, f64)> {
        self.dims
            .iter()
            .zip(x)
            .map(|(d, u)| {
                let value = d.min + u.clamp(0.0, 1.0) * (d.max - d.min);
                (
                    d.name.clone(),
                    if d.integer { value.round() } else { value },
                )
            })
            .collect()
    }

    fn unit(d: &Dim, value: f64) -> f64 {
        ((value - d.min) / (d.max - d.min)).clamp(0.0, 1.0)
    }

    fn candidate(&self, values: &[(String, f64)]) -> Result<BotBuild, String> {
        builds::with_params(&self.base, values)
    }

    fn eval(&self, build: &BotBuild, (first, count): (u64, u64)) -> Result<Eval, String> {
        let rows = self.bench.play(build, first, count)?;
        Ok(Eval {
            fitness: self.fitness.score(&rows, self.bench.setups())?,
            metrics: mean_metrics(&rows),
        })
    }

    fn eval_values(&self, values: &[(String, f64)], seeds: (u64, u64)) -> Result<Eval, String> {
        self.eval(&self.candidate(values)?, seeds)
    }

    fn base_values(&self) -> Vec<(String, f64)> {
        self.dims.iter().map(|d| (d.name.clone(), d.base)).collect()
    }

    fn scored(&self, values: &[(String, f64)]) -> Result<Scored, String> {
        Ok(Scored {
            values: values.iter().cloned().collect(),
            train: self.eval_values(values, self.train)?,
            holdout: self.eval_values(values, self.holdout)?,
        })
    }

    /// Move one parameter at a time over its range, the others at the base.
    pub fn sweep(&self, threads: Option<usize>) -> Result<SweepReport, String> {
        thread_pool(threads)?.install(|| {
            let base = self.eval_values(&self.base_values(), self.train)?;
            let mut params = Vec::new();
            for (i, d) in self.dims.iter().enumerate() {
                let mut grid: Vec<f64> = (0..self.sweep.steps)
                    .map(|k| d.min + (d.max - d.min) * k as f64 / (self.sweep.steps - 1) as f64)
                    .map(|v| if d.integer { v.round() } else { v })
                    .collect();
                grid.dedup();
                let mut points = Vec::new();
                for value in grid {
                    let mut values = self.base_values();
                    values[i].1 = value;
                    points.push(SweepPoint {
                        value,
                        eval: self.eval_values(&values, self.train)?,
                    });
                }
                let dead = points.iter().all(|p| p.eval == points[0].eval);
                params.push(ParamSweep {
                    name: d.name.clone(),
                    base: d.base,
                    dead,
                    points,
                });
            }
            Ok(SweepReport { base, params })
        })
    }

    /// Seeded CMA-ES over all parameters, scored on the train seeds; the
    /// winner and the base are then scored on the train and held-out seeds.
    pub fn search(&self, threads: Option<usize>) -> Result<SearchReport, String> {
        thread_pool(threads)?.install(|| {
            let memo: Mutex<HashMap<Vec<u64>, f64>> = Mutex::new(HashMap::new());
            // Integer parameters make many points equal: score each build once.
            let fitness = |x: &DVector<f64>| -> f64 {
                let values = self.decode(x.as_slice());
                let key: Vec<u64> = values.iter().map(|(_, v)| v.to_bits()).collect();
                if let Some(&known) = memo.lock().expect("memo lock").get(&key) {
                    return known;
                }
                let score = self
                    .eval_values(&values, self.train)
                    .expect("validated candidate")
                    .fitness;
                memo.lock().expect("memo lock").insert(key, score);
                score
            };
            let start: Vec<f64> = self.dims.iter().map(|d| Self::unit(d, d.base)).collect();
            let mut options = CMAESOptions::new(start, self.search.sigma)
                .mode(Mode::Maximize)
                .seed(self.seed)
                .max_generations(self.search.generations);
            if let Some(population) = self.search.population {
                options = options.population_size(population);
            }
            let mut cma = options
                .build(&fitness)
                .map_err(|e| format!("cmaes: {e:?}"))?;
            let mut history = Vec::new();
            let done = loop {
                let done = cma.next_parallel();
                let best = cma.current_best_individual().map(|b| Generation {
                    generation: cma.generation(),
                    evals: cma.function_evals(),
                    best_fitness: b.value,
                    best: self.decode(b.point.as_slice()).into_iter().collect(),
                    mean: self.decode(cma.mean().as_slice()).into_iter().collect(),
                });
                history.extend(best);
                if let Some(done) = done {
                    break done;
                }
            };
            let winner = done
                .overall_best
                .ok_or("cmaes found no valid point (fitness was NaN)")?;
            let best_values = self.decode(winner.point.as_slice());
            let best = self.scored(&best_values)?;
            let build = self.candidate(&best_values)?;
            Ok(SearchReport {
                seed: self.seed,
                train_seeds: self.train,
                holdout_seeds: self.holdout,
                stopped: done.reasons.iter().map(|r| format!("{r:?}")).collect(),
                base: self.scored(&self.base_values())?,
                best,
                history,
                build: builds::roster_entry(&build)?,
            })
        })
    }
}

/// One parameter vector scored on both seed sets.
#[derive(Debug, Serialize)]
pub struct Scored {
    pub values: BTreeMap<String, f64>,
    pub train: Eval,
    pub holdout: Eval,
}

#[derive(Debug, Serialize)]
pub struct SweepPoint {
    pub value: f64,
    pub eval: Eval,
}

#[derive(Debug, Serialize)]
pub struct ParamSweep {
    pub name: String,
    pub base: f64,
    /// No metric moved anywhere along the range.
    pub dead: bool,
    pub points: Vec<SweepPoint>,
}

#[derive(Debug, Serialize)]
pub struct SweepReport {
    pub base: Eval,
    pub params: Vec<ParamSweep>,
}

#[derive(Debug, Serialize)]
pub struct Generation {
    pub generation: usize,
    /// Fitness evaluations asked for so far (repeated builds included).
    pub evals: usize,
    pub best_fitness: f64,
    pub best: BTreeMap<String, f64>,
    pub mean: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize)]
pub struct SearchReport {
    pub seed: u64,
    pub train_seeds: (u64, u64),
    pub holdout_seeds: (u64, u64),
    /// Why CMA-ES stopped.
    pub stopped: Vec<String>,
    pub base: Scored,
    pub best: Scored,
    pub history: Vec<Generation>,
    /// The best build as a `[[build]]` entry (roster and suite vocabulary).
    pub build: String,
}

/// A parameter value as stored: whole numbers plain, others as `f32`.
fn param(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value}")
    } else {
        format!("{}", value as f32)
    }
}

fn cell(value: Option<f64>) -> String {
    value.map_or("-".into(), |v| format!("{v:.3}"))
}

/// Metric columns of the tables: everything but the round count.
fn shown() -> impl Iterator<Item = &'static str> {
    METRICS.into_iter().filter(|m| *m != "rounds")
}

fn eval_cells(eval: &Eval) -> String {
    let mut out = format!(" {:.3} |", eval.fitness);
    for name in shown() {
        out += &format!(" {} |", cell(eval.metrics[name]));
    }
    out
}

fn header(first: &str) -> String {
    let names: Vec<&str> = shown().collect();
    format!(
        "| {first} | fitness | {} |\n|---|---:|{}\n",
        names.join(" | "),
        "---:|".repeat(names.len())
    )
}

impl SweepReport {
    pub fn markdown(&self) -> String {
        let mut out = String::from("## Sweep (train seeds, other parameters at the base)\n\n");
        out += &header("base");
        out += &format!("| base |{}\n\n", eval_cells(&self.base));
        out += "| parameter | base | fitness range | verdict |\n|---|---:|---:|---|\n";
        for p in &self.params {
            let fits = p.points.iter().map(|q| q.eval.fitness);
            let range = fits.clone().fold(f64::MIN, f64::max) - fits.fold(f64::MAX, f64::min);
            let verdict = if p.dead {
                "DEAD: no metric moved"
            } else {
                "moves"
            };
            out += &format!(
                "| {} | {} | {range:.3} | {verdict} |\n",
                p.name,
                param(p.base)
            );
        }
        for p in &self.params {
            out += &format!(
                "\n### {}{}\n\n",
                p.name,
                if p.dead { " (dead knob)" } else { "" }
            );
            out += &header(&p.name);
            for q in &p.points {
                out += &format!("| {} |{}\n", param(q.value), eval_cells(&q.eval));
            }
        }
        out
    }
}

impl SearchReport {
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "## Search (seed {}, stopped: {})\n\nTrain seeds {}..+{}, held-out seeds {}..+{}.\n\n",
            self.seed,
            self.stopped.join(", "),
            self.train_seeds.0,
            self.train_seeds.1,
            self.holdout_seeds.0,
            self.holdout_seeds.1
        );
        out += "| generation | evals | best fitness (train) |\n|---:|---:|---:|\n";
        for g in &self.history {
            out += &format!(
                "| {} | {} | {:.3} |\n",
                g.generation, g.evals, g.best_fitness
            );
        }
        out += "\n### Scores\n\n";
        out += &header("build");
        for (name, scored) in [("base", &self.base), ("best", &self.best)] {
            out += &format!("| {name} train |{}\n", eval_cells(&scored.train));
            out += &format!("| {name} held-out |{}\n", eval_cells(&scored.holdout));
        }
        out += "\n### Parameters\n\n| parameter | base | best |\n|---|---:|---:|\n";
        for (name, value) in &self.best.values {
            out += &format!(
                "| {name} | {} | {} |\n",
                param(self.base.values[name]),
                param(*value)
            );
        }
        out += &format!(
            "\nBest build (a roster or suite entry):\n\n```toml\n{}```\n",
            self.build
        );
        out
    }
}
