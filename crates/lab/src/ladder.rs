//! `lab ladder`: a difficulty ladder from one build.
//!
//! The build is degraded along its budget knobs (slower reaction, shorter
//! horizon, less variety, more noise) in `candidates` steps from itself
//! (step 0) to the weakest budget (last step). Every step is measured against
//! a fixed pool; `rungs` of them are picked evenly spaced in measured
//! strength, and every pair of rungs plays head to head to check that a
//! higher rung beats a lower one. The result is a roster file (`bots::roster`).
//!
//! ```toml
//! seeds = 6                   # seeds of the strength measurement (first_seed = 1)
//! check_seeds = 6             # seeds of each head-to-head (check_first = 100000)
//! rungs = 5
//! candidates = 12             # at least `rungs`
//! strength = "win_rate"       # a metric of `tournament::METRICS` ("-name": lower is
//!                             # better), or "duels": round-robin of the candidates
//! name = "Cadet"              # rungs are "Cadet 1" (weakest) .. "Cadet 5"
//! degrade_to = 500            # authoring preset whose budget is the weakest end
//! roster = "builtin"          # pool, as in a tune file (not played by "duels")
//!
//! [degrade]                   # optional: weakest value of single knobs
//! noise = 0.9
//!
//! [base]                      # the build to degrade, see `crate::builds`
//! name = "Ace"
//! bot = "trapper"
//!
//! [[setup]]                   # as in a suite; last_standing for head-to-heads
//! ```

use std::path::{Path, PathBuf};

use bots::config::BotBuild;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bench::{Bench, mean_metrics};
use crate::builds::{self, BuildSpec, Param};
use crate::scenario::TierPin;
use crate::tournament::{METRICS, SetupSpec, thread_pool};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    seeds: u64,
    #[serde(default = "one")]
    first_seed: u64,
    #[serde(default)]
    check_seeds: Option<u64>,
    #[serde(default = "check_first")]
    check_first: u64,
    rungs: usize,
    candidates: usize,
    #[serde(default = "win_rate")]
    strength: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "weakest")]
    degrade_to: u32,
    #[serde(default)]
    degrade: toml::Table,
    #[serde(default)]
    tier: Option<TierPin>,
    base: BuildSpec,
    #[serde(default)]
    roster: Option<PathBuf>,
    #[serde(default, rename = "pool")]
    pool_builds: Vec<BuildSpec>,
    #[serde(rename = "setup")]
    setups: Vec<SetupSpec>,
}

fn one() -> u64 {
    1
}

fn check_first() -> u64 {
    100_000
}

/// The strength measure that needs no pool: every candidate plays every
/// other head to head, and its strength is the share of decided rounds it won.
const DUELS: &str = "duels";

fn win_rate() -> String {
    "win_rate".into()
}

fn weakest() -> u32 {
    500
}

/// A loaded ladder file.
pub struct Ladder {
    seeds: (u64, u64),
    check: (u64, u64),
    rungs: usize,
    candidates: usize,
    strength: String,
    prefix: String,
    base: BotBuild,
    /// Knob name, strongest value (the base's), weakest value.
    knobs: Vec<(String, Param, Param)>,
    bench: Bench,
}

impl Ladder {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new(""));
        Self::parse(&text, dir).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str, dir: &Path) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.strength != DUELS && !METRICS.contains(&file.strength.trim_start_matches('-')) {
            return Err(format!("strength: unknown metric {:?}", file.strength));
        }
        if file.rungs < 2 || file.candidates < file.rungs || file.seeds == 0 {
            return Err("give rungs >= 2, candidates >= rungs and seeds >= 1".into());
        }
        let base = file.base.build()?;
        let own = builds::params(&base)?;
        let mut weakest = builds::preset_knobs(file.degrade_to)?;
        for (name, value) in &file.degrade {
            let param = weakest
                .get_mut(name)
                .ok_or_else(|| format!("degrade: {name:?} is not a knob"))?;
            param.value = value
                .as_float()
                .or_else(|| value.as_integer().map(|v| v as f64))
                .ok_or_else(|| format!("degrade.{name}: expected a number"))?;
        }
        let mut knobs = Vec::new();
        for name in builds::knob_names()? {
            let (Some(best), Some(worst)) = (own.get(&name), weakest.remove(&name)) else {
                continue;
            };
            knobs.push((name, best.clone(), worst));
        }
        let check = file.check_seeds.unwrap_or(file.seeds);
        let pool = builds::resolve(file.roster.as_deref(), &file.pool_builds, dir)?;
        let bench = Bench::new(file.setups, file.tier, pool)?;
        if !bench.setups()[0].rules.end.last_standing {
            return Err("the first setup needs last_standing rules (head-to-head checks)".into());
        }
        Ok(Self {
            seeds: (file.first_seed, file.seeds),
            check: (file.check_first, check),
            rungs: file.rungs,
            candidates: file.candidates,
            strength: file.strength,
            prefix: file.name.unwrap_or_else(|| base.name.clone()),
            base,
            knobs,
            bench,
        })
    }

    /// Strength of every candidate (at degradations `ts`) from a round robin:
    /// the share of decided rounds it won against the other candidates.
    fn round_robin(&self, ts: &[f64]) -> Result<Vec<f64>, String> {
        let builds = ts
            .iter()
            .enumerate()
            .map(|(k, &t)| self.degraded(&format!("step-{k}"), t))
            .collect::<Result<Vec<_>, _>>()?;
        let pairs: Vec<(usize, usize)> = (0..builds.len())
            .flat_map(|a| (a + 1..builds.len()).map(move |b| (a, b)))
            .collect();
        let results = pairs
            .par_iter()
            .map(|&(a, b)| {
                self.bench
                    .duel(&builds[a], &builds[b], self.seeds.0, self.seeds.1)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut won = vec![0.0; builds.len()];
        let mut decided = vec![0.0; builds.len()];
        for (&(a, b), [wa, wb]) in pairs.iter().zip(results) {
            won[a] += f64::from(wa);
            won[b] += f64::from(wb);
            decided[a] += f64::from(wa + wb);
            decided[b] += f64::from(wa + wb);
        }
        Ok(won
            .iter()
            .zip(&decided)
            .map(|(w, d)| if *d > 0.0 { w / d } else { 0.5 })
            .collect())
    }

    /// The base build degraded by `t` in `0..=1`: every knob that far along
    /// its way to the weakest value.
    fn degraded(&self, name: &str, t: f64) -> Result<BotBuild, String> {
        let set: Vec<(String, f64)> = self
            .knobs
            .iter()
            .map(|(knob, best, worst)| (knob.clone(), best.value + t * (worst.value - best.value)))
            .collect();
        let mut build = builds::with_params(&self.base, &set)?;
        build.name = name.into();
        Ok(build)
    }

    pub fn run(&self, threads: Option<usize>) -> Result<LadderReport, String> {
        thread_pool(threads)?.install(|| self.run_on_pool())
    }

    fn run_on_pool(&self) -> Result<LadderReport, String> {
        let ts: Vec<f64> = (0..self.candidates)
            .map(|k| k as f64 / (self.candidates - 1) as f64)
            .collect();
        let strengths = if self.strength == DUELS {
            self.round_robin(&ts)?
        } else {
            let name = self.strength.trim_start_matches('-');
            let sign = if name == self.strength { 1.0 } else { -1.0 };
            let mut out = Vec::new();
            for (k, &t) in ts.iter().enumerate() {
                let build = self.degraded(&format!("step-{k}"), t)?;
                let rows = self.bench.play(&build, self.seeds.0, self.seeds.1)?;
                let value = mean_metrics(&rows)[name].ok_or_else(|| {
                    format!("strength {:?} is undefined in a setup", self.strength)
                })?;
                out.push(value * sign);
            }
            out
        };
        let steps: Vec<Step> = ts
            .iter()
            .zip(strengths)
            .map(|(&t, strength)| Step { t, strength })
            .collect();

        // Evenly spaced targets between the weakest and strongest measured.
        let low = steps.iter().map(|s| s.strength).fold(f64::MAX, f64::min);
        let high = steps.iter().map(|s| s.strength).fold(f64::MIN, f64::max);
        let mut picked: Vec<usize> = Vec::new();
        for j in 0..self.rungs {
            let target = low + (high - low) * j as f64 / (self.rungs - 1) as f64;
            let nearest = (0..steps.len())
                .filter(|k| !picked.contains(k))
                .min_by(|&a, &b| {
                    let (da, db) = (
                        (steps[a].strength - target).abs(),
                        (steps[b].strength - target).abs(),
                    );
                    da.total_cmp(&db).then(b.cmp(&a))
                })
                .ok_or("not enough distinct candidates")?;
            picked.push(nearest);
        }
        // Weakest first; equal strength: the more degraded step is lower.
        picked.sort_by(|&a, &b| {
            steps[a]
                .strength
                .total_cmp(&steps[b].strength)
                .then(b.cmp(&a))
        });

        let mut rungs = Vec::new();
        for (i, &k) in picked.iter().enumerate() {
            let name = format!("{} {}", self.prefix, i + 1);
            rungs.push(Rung {
                build: self.degraded(&name, steps[k].t)?,
                step: k,
                strength: steps[k].strength,
            });
        }

        let mut pairs = Vec::new();
        for lo in 0..rungs.len() {
            for hi in lo + 1..rungs.len() {
                let [wins_hi, wins_lo] = self.bench.duel(
                    &rungs[hi].build,
                    &rungs[lo].build,
                    self.check.0,
                    self.check.1,
                )?;
                pairs.push(Pair {
                    lower: lo,
                    higher: hi,
                    wins_higher: wins_hi,
                    wins_lower: wins_lo,
                });
            }
        }
        Ok(LadderReport {
            metric: self.strength.clone(),
            steps,
            rungs,
            pairs,
        })
    }
}

/// One degradation step of the base, measured.
#[derive(Debug, Serialize)]
pub struct Step {
    /// How far along the way to the weakest budget, `0..=1`.
    pub t: f64,
    pub strength: f64,
}

#[derive(Debug)]
pub struct Rung {
    pub build: BotBuild,
    /// Index of the step it came from.
    pub step: usize,
    pub strength: f64,
}

/// A head-to-head: rounds won by the higher and the lower rung.
#[derive(Debug, Serialize)]
pub struct Pair {
    pub lower: usize,
    pub higher: usize,
    pub wins_higher: u32,
    pub wins_lower: u32,
}

#[derive(Debug)]
pub struct LadderReport {
    pub metric: String,
    pub steps: Vec<Step>,
    pub rungs: Vec<Rung>,
    pub pairs: Vec<Pair>,
}

impl LadderReport {
    /// Whether every higher rung won more rounds than every lower one.
    pub fn monotonic(&self) -> bool {
        self.pairs.iter().all(|p| p.wins_higher > p.wins_lower)
    }

    /// The report as JSON: steps, rungs and head-to-heads.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "metric": self.metric,
            "monotonic": self.monotonic(),
            "steps": self.steps,
            "rungs": self.rungs.iter().map(|r| serde_json::json!({
                "name": r.build.name,
                "step": r.step,
                "strength": r.strength,
                "knobs": r.build.knobs,
            })).collect::<Vec<_>>(),
            "pairs": self.pairs,
        })
    }

    /// The rungs as a roster file, weakest first.
    pub fn roster(&self) -> Result<String, String> {
        let header = format!(
            "Difficulty ladder written by `lab ladder`, weakest rung first.\n\
             Strength = {} against the reference pool; head-to-head check: {}.",
            self.metric,
            if self.monotonic() {
                "passed (every higher rung won more rounds than every lower one)"
            } else {
                "FAILED (see the report)"
            }
        );
        let builds: Vec<BotBuild> = self.rungs.iter().map(|r| r.build.clone()).collect();
        builds::roster_file(&builds, &header)
    }

    pub fn markdown(&self) -> String {
        let mut out = format!("## Candidates (strength = {})\n\n", self.metric);
        out += "| step | degradation | strength |\n|---:|---:|---:|\n";
        for (k, s) in self.steps.iter().enumerate() {
            out += &format!("| {k} | {:.2} | {:.3} |\n", s.t, s.strength);
        }
        out += "\n## Rungs (weakest first)\n\n| rung | step | strength | knobs |\n|---|---:|---:|---|\n";
        for r in &self.rungs {
            let knobs = &r.build.knobs;
            out += &format!(
                "| {} | {} | {:.3} | reaction {} horizon {} variety {} noise {:.2} |\n",
                r.build.name,
                r.step,
                r.strength,
                knobs.reaction_ticks,
                knobs.horizon_ticks,
                knobs.variety,
                knobs.noise
            );
        }
        out += "\n## Head to head (rounds won: higher rung / lower rung)\n\n";
        for p in &self.pairs {
            let ok = if p.wins_higher > p.wins_lower {
                "ok"
            } else {
                "NOT MONOTONIC"
            };
            out += &format!(
                "- {} vs {}: {} / {} {ok}\n",
                self.rungs[p.higher].build.name,
                self.rungs[p.lower].build.name,
                p.wins_higher,
                p.wins_lower
            );
        }
        out += &format!(
            "\nMonotonic: {}\n",
            if self.monotonic() { "yes" } else { "NO" }
        );
        out
    }
}
