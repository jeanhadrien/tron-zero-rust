//! `lab tune`: sweep the knobs and weights of a bot build for a game setup.
//!
//! A tune file (TOML) names a base build, the parameters to sweep with their
//! ranges, the setups, the reference pool to play against, the seeds and the
//! fitness (see `bench`):
//!
//! ```toml
//! seeds = 4                   # seeds first_seed.. (default first_seed = 1)
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
//! ```
//!
//! The sweep moves one parameter at a time, the others at the base, and
//! reports how every metric follows (a parameter that moves nothing is a dead
//! knob). Fitness is deterministic for a build and a seed set, so the report
//! does not depend on the thread count.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bots::config::BotBuild;
use serde::{Deserialize, Serialize};

use crate::bench::{Bench, Fitness, mean_metrics};
use crate::builds::{self, BuildSpec};
use crate::scenario::TierPin;
use crate::tournament::{METRICS, SetupSpec, thread_pool};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    seeds: u64,
    #[serde(default = "one")]
    first_seed: u64,
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
}

fn one() -> u64 {
    1
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

/// One swept parameter, resolved against the base build.
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
    seeds: (u64, u64),
    base: BotBuild,
    dims: Vec<Dim>,
    bench: Bench,
    fitness: Fitness,
    sweep: SweepSpec,
}

/// A candidate's score.
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
        if file.seeds == 0 || file.sweep.steps < 2 {
            return Err("seeds >= 1 and sweep.steps >= 2".into());
        }
        let pool = builds::resolve(
            file.default_roster,
            file.roster.as_deref(),
            &file.pool_builds,
            dir,
        )?;
        let bench = Bench::new(file.setups, file.tier, pool)?;
        Ok(Self {
            seeds: (file.first_seed, file.seeds),
            base,
            dims,
            bench,
            fitness: file.fitness,
            sweep: file.sweep,
        })
    }

    fn eval_values(&self, values: &[(String, f64)]) -> Result<Eval, String> {
        let build = builds::with_params(&self.base, values)?;
        let rows = self.bench.play(&build, self.seeds.0, self.seeds.1)?;
        Ok(Eval {
            fitness: self.fitness.score(&rows, self.bench.setups())?,
            metrics: mean_metrics(&rows),
        })
    }

    fn base_values(&self) -> Vec<(String, f64)> {
        self.dims.iter().map(|d| (d.name.clone(), d.base)).collect()
    }

    /// Move one parameter at a time over its range, the others at the base.
    pub fn sweep(&self, threads: Option<usize>) -> Result<SweepReport, String> {
        thread_pool(threads)?.install(|| {
            let base = self.eval_values(&self.base_values())?;
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
                        eval: self.eval_values(&values)?,
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

    /// The base build with the single parameter value that scored best in
    /// `report` (the base itself when nothing beat it), as a roster entry.
    pub fn best_entry(&self, report: &SweepReport) -> Result<String, String> {
        let mut best = (report.base.fitness, None);
        for p in &report.params {
            for q in &p.points {
                if q.eval.fitness > best.0 {
                    best = (q.eval.fitness, Some((p.name.clone(), q.value)));
                }
            }
        }
        let set: Vec<(String, f64)> = best.1.into_iter().collect();
        builds::roster_entry(&builds::with_params(&self.base, &set)?)
    }
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
        let mut out = String::from("## Sweep (other parameters at the base)\n\n");
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
