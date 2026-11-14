//! Tournaments: bot builds play many seeded matches and get a table of
//! metrics, one column each, no combined score.
//!
//! A suite file (TOML) lists the builds, the setups (rider count, match
//! rules, arena) and a seed count:
//!
//! ```toml
//! seeds = 20                  # matches per setup and lineup: seeds first..first+20
//! first_seed = 1              # optional
//! default_roster = true       # start from bots::config::default_builds()
//! tier = "full"               # optional: pin the bots' level of detail
//!
//! [[build]]                   # optional extra builds, as scenario riders
//! name = "Tuned"
//! bot = "trapper"
//! difficulty = 2500
//! [build.knobs]
//! mistake_prob = 0.0
//!
//! [[setup]]
//! name = "ffa4"
//! riders = 4
//! arena = [2000, 2000]        # optional
//! tier = "full"               # optional, wins over the suite's
//! [setup.match]               # shared::MatchRules; time_limit is required
//! respawn = { after = 240 }
//! [setup.match.end]
//! time_limit = 3600
//! ```
//!
//! Every lineup of builds is played over every seed. When a setup seats more
//! riders than there are builds, builds repeat around the table; with fewer
//! seats, every subset of builds plays. Either way each lineup is rotated
//! through all seats, so spawn luck washes out. Matches run in parallel but
//! results are folded in a fixed order: the report does not depend on the
//! thread count (the `timing` part aside, it is wall-clock).

use std::collections::BTreeMap;
use std::path::Path;

use bots::config::{BotBuild, KnobOverrides, default_builds};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use shared::{MatchRules, RoundEnd, TICK_HZ};

use crate::scenario::{RiderSpec, Scenario, TierPin, bot_build};
use crate::sim::{EventKind, Lab};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    /// Seeds per setup and lineup.
    pub seeds: u64,
    #[serde(default = "first_seed")]
    pub first_seed: u64,
    #[serde(default)]
    pub default_roster: bool,
    #[serde(default)]
    pub tier: Option<TierPin>,
    #[serde(default, rename = "build")]
    pub extra_builds: Vec<BuildSpec>,
    #[serde(rename = "setup")]
    pub setups: Vec<SetupSpec>,
    /// Resolved builds, filled by `load`/`parse`.
    #[serde(skip)]
    pub builds: Vec<BotBuild>,
}

fn first_seed() -> u64 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    pub name: String,
    pub bot: String,
    #[serde(default = "default_difficulty")]
    pub difficulty: u32,
    #[serde(default)]
    pub knobs: KnobOverrides,
}

fn default_difficulty() -> u32 {
    1500
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupSpec {
    pub name: String,
    pub riders: usize,
    #[serde(default)]
    pub arena: Option<[f32; 2]>,
    #[serde(default)]
    pub tier: Option<TierPin>,
    #[serde(rename = "match")]
    pub rules: MatchRules,
}

impl Suite {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut suite: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if suite.default_roster {
            suite.builds = default_builds();
        }
        for spec in &suite.extra_builds {
            let build = bot_build(&spec.name, &spec.bot, spec.difficulty, &spec.knobs)?;
            suite.builds.push(build);
        }
        let mut names = std::collections::HashSet::new();
        if suite.builds.is_empty() {
            return Err("no builds: set default_roster or add [[build]]".into());
        }
        if let Some(build) = suite.builds.iter().find(|b| !names.insert(b.name.as_str())) {
            return Err(format!("duplicate build name {:?}", build.name));
        }
        if suite.seeds == 0 || suite.setups.is_empty() {
            return Err("give seeds >= 1 and at least one [[setup]]".into());
        }
        for setup in &suite.setups {
            setup.rules.validate()?;
            if setup.riders < 2 {
                return Err(format!("{}: riders must be at least 2", setup.name));
            }
            if setup.rules.end.time_limit.is_none() {
                return Err(format!(
                    "{}: tournament setups need a time_limit",
                    setup.name
                ));
            }
        }
        Ok(suite)
    }

    /// Play every build without mistakes (`mistake_prob = 0`).
    pub fn without_mistakes(&mut self) {
        for build in &mut self.builds {
            build.knobs.mistake_prob = 0.0;
        }
    }
}

/// One match to play: a setup, a spawn seed and who sits where.
struct Game {
    setup: usize,
    seed: u64,
    /// Build index per seat.
    seats: Vec<usize>,
}

/// Which builds sit in which seats: every subset of `n` builds (or the whole
/// roster repeated around the table when `n >= m`), rotated through all seats.
fn lineups(m: usize, n: usize) -> Vec<Vec<usize>> {
    if n >= m {
        return (0..m)
            .map(|shift| (0..n).map(|seat| (seat + shift) % m).collect())
            .collect();
    }
    let mut out = Vec::new();
    let mut subset: Vec<usize> = (0..n).collect();
    loop {
        for turn in 0..n {
            out.push((0..n).map(|seat| subset[(seat + turn) % n]).collect());
        }
        // Next combination in lexicographic order.
        let Some(i) = (0..n).rev().find(|&i| subset[i] < m - n + i) else {
            return out;
        };
        subset[i] += 1;
        for j in i + 1..n {
            subset[j] = subset[j - 1] + 1;
        }
    }
}

fn games(suite: &Suite) -> Vec<Game> {
    let mut games = Vec::new();
    for (setup, spec) in suite.setups.iter().enumerate() {
        for seats in lineups(suite.builds.len(), spec.riders) {
            for seed in suite.first_seed..suite.first_seed + suite.seeds {
                games.push(Game {
                    setup,
                    seed,
                    seats: seats.clone(),
                });
            }
        }
    }
    games
}

/// What one seat did in one match.
struct SeatResult {
    build: usize,
    rounds: u32,
    wins: u32,
    kills_on: BTreeMap<usize, u32>,
    wall: u32,
    own_trail: u32,
    trail: u32,
    alive_ticks: u64,
    idle_ticks: u64,
    rubber_ticks: u64,
    speed_sum: f64,
    lifetimes: Vec<u32>,
    think_nanos: Vec<u32>,
}

/// One finished match: every seat's numbers and how its rounds ended.
struct GameResult {
    seats: Vec<SeatResult>,
    /// Rounds that hit the time limit with several riders alive.
    timeouts: u32,
    /// Rounds in which the last riders died on the same tick.
    draws: u32,
}

fn seat_name(seat: usize) -> String {
    format!("s{seat}")
}

fn play(suite: &Suite, game: &Game) -> GameResult {
    let setup = &suite.setups[game.setup];
    let rules = setup.rules;
    let time_limit = rules.end.time_limit.expect("checked by Suite::parse");
    let riders = game
        .seats
        .iter()
        .enumerate()
        .map(|(seat, &build)| RiderSpec {
            name: seat_name(seat),
            at: None,
            heading: None,
            speed: 1.0,
            rubber: shared::BASE_RUBBER,
            script: Vec::new(),
            bot: Some("tournament".into()),
            difficulty: 0,
            knobs: KnobOverrides::default(),
            respawn_after: None,
            prebuilt: Some(suite.builds[build].clone()),
        })
        .collect();
    let scenario = Scenario {
        until: rules.rounds * time_limit,
        arena: setup.arena,
        walls: Vec::new(),
        seed: game.seed,
        rules: Some(rules),
        tier: setup.tier.or(suite.tier),
        riders,
    };
    let mut lab = Lab::with_seed(&scenario, game.seed);
    lab.run(scenario.until, 0);

    let seat_of = |name: &str| name.strip_prefix('s').and_then(|n| n.parse::<usize>().ok());
    let mut seats: Vec<SeatResult> = game
        .seats
        .iter()
        .zip(lab.stats().into_iter().zip(lab.think_nanos()))
        .enumerate()
        .map(|(seat, (&build, (stats, nanos)))| SeatResult {
            build,
            rounds: lab.rounds().len() as u32,
            wins: lab
                .rounds()
                .iter()
                .filter(|r| r.winner == Some(seat))
                .count() as u32,
            kills_on: BTreeMap::new(),
            wall: 0,
            own_trail: 0,
            trail: 0,
            alive_ticks: stats.alive_ticks,
            idle_ticks: stats.idle_ticks,
            rubber_ticks: stats.rubber_ticks,
            speed_sum: stats.speed_sum,
            lifetimes: stats.lifetimes.clone(),
            think_nanos: nanos.to_vec(),
        })
        .collect();
    for event in lab.events() {
        let EventKind::Death { cause, .. } = &event.kind else {
            continue;
        };
        let victim = seat_of(&event.rider).expect("seat names");
        match cause.strip_prefix("trail:") {
            None if cause == "wall" => seats[victim].wall += 1,
            None => seats[victim].own_trail += 1,
            Some(killer) => {
                seats[victim].trail += 1;
                let killer = seat_of(killer).expect("seat names");
                let victim_build = seats[victim].build;
                *seats[killer].kills_on.entry(victim_build).or_default() += 1;
            }
        }
    }
    GameResult {
        seats,
        timeouts: lab
            .rounds()
            .iter()
            .filter(|r| r.reason == RoundEnd::TimeLimit)
            .count() as u32,
        draws: lab
            .rounds()
            .iter()
            .filter(|r| r.reason == RoundEnd::LastStanding && r.winner.is_none())
            .count() as u32,
    }
}

/// Everything measured for one build in one setup, before it becomes a row.
#[derive(Default)]
struct Totals {
    rounds: u32,
    wins: u32,
    kills: u32,
    wall: u32,
    own_trail: u32,
    trail: u32,
    alive_ticks: u64,
    idle_ticks: u64,
    rubber_ticks: u64,
    speed_sum: f64,
    lifetimes: Vec<u32>,
}

/// Metrics of one build in one setup. Rates are `None` when undefined.
#[derive(Debug, Serialize)]
pub struct BuildRow {
    pub build: String,
    /// Rounds played (matches, for a one-round setup).
    pub rounds: u32,
    /// Share of rounds won; only for last-rider-standing setups.
    pub win_rate: Option<f64>,
    pub kills_per_min: Option<f64>,
    pub deaths_wall: u32,
    pub deaths_own_trail: u32,
    pub deaths_trail: u32,
    /// (wall + own trail deaths) / all deaths.
    pub suicide_share: Option<f64>,
    pub median_life_ticks: Option<f64>,
    pub idle_share: Option<f64>,
    pub rubber_share: Option<f64>,
    pub mean_speed: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct SetupReport {
    pub name: String,
    pub riders: usize,
    pub matches: u32,
    /// Rounds played over all matches, and how many ended on the time limit
    /// (several riders alive) or as a draw (last riders died together).
    pub rounds_played: u32,
    pub timeouts: u32,
    pub draws: u32,
    pub last_standing: bool,
    pub rows: Vec<BuildRow>,
    /// Build names, the order of `kills` rows and columns.
    pub builds: Vec<String>,
    /// `kills[a][b]`: kills of build `a` on build `b`.
    pub kills: Vec<Vec<u32>>,
}

/// Wall-clock cost of `Brain::think` per build. Not deterministic.
#[derive(Debug, Serialize)]
pub struct Timing {
    pub threads: usize,
    pub builds: Vec<BuildTiming>,
}

#[derive(Debug, Serialize)]
pub struct BuildTiming {
    pub build: String,
    pub decisions: usize,
    pub mean_us: f64,
    pub p99_us: f64,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub seeds: u64,
    pub setups: Vec<SetupReport>,
    pub timing: Timing,
}

fn ratio(num: f64, den: f64) -> Option<f64> {
    (den > 0.0).then(|| num / den)
}

fn median(values: &[u32]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(f64::from(sorted[mid])),
        _ => Some(f64::from(sorted[mid - 1] + sorted[mid]) / 2.0),
    }
}

/// Value at quantile `q` of an ascending-sorted slice (nearest rank).
fn quantile(sorted: &[u32], q: f64) -> f64 {
    let rank = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len());
    f64::from(sorted[rank - 1])
}

/// Play the whole suite on `threads` threads (`None`: one per core).
pub fn run(suite: &Suite, threads: Option<usize>) -> Result<Report, String> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.unwrap_or(0))
        .build()
        .map_err(|e| e.to_string())?;
    let games = games(suite);
    // Indexed collect keeps game order, so folding below is thread-independent.
    let results: Vec<GameResult> =
        pool.install(|| games.par_iter().map(|game| play(suite, game)).collect());

    let m = suite.builds.len();
    let mut totals: Vec<Vec<Totals>> = (0..suite.setups.len())
        .map(|_| (0..m).map(|_| Totals::default()).collect())
        .collect();
    let mut kills = vec![vec![vec![0u32; m]; m]; suite.setups.len()];
    let mut matches = vec![0u32; suite.setups.len()];
    let mut nanos: Vec<Vec<u32>> = vec![Vec::new(); m];
    let mut timeouts = vec![0u32; suite.setups.len()];
    let mut draws = vec![0u32; suite.setups.len()];
    let mut rounds_played = vec![0u32; suite.setups.len()];
    for (game, result) in games.iter().zip(results) {
        timeouts[game.setup] += result.timeouts;
        rounds_played[game.setup] += result.seats[0].rounds;
        draws[game.setup] += result.draws;
        matches[game.setup] += 1;
        for seat in result.seats {
            let t = &mut totals[game.setup][seat.build];
            t.rounds += seat.rounds;
            t.wins += seat.wins;
            t.wall += seat.wall;
            t.own_trail += seat.own_trail;
            t.trail += seat.trail;
            t.alive_ticks += seat.alive_ticks;
            t.idle_ticks += seat.idle_ticks;
            t.rubber_ticks += seat.rubber_ticks;
            t.speed_sum += seat.speed_sum;
            t.lifetimes.extend(seat.lifetimes);
            for (victim, n) in seat.kills_on {
                kills[game.setup][seat.build][victim] += n;
                t.kills += n;
            }
            nanos[seat.build].extend(seat.think_nanos);
        }
    }

    let setups = suite
        .setups
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let last_standing = spec.rules.end.last_standing;
            let rows = suite
                .builds
                .iter()
                .zip(&totals[i])
                .filter(|(_, t)| t.rounds > 0)
                .map(|(build, t)| {
                    let deaths = f64::from(t.wall + t.own_trail + t.trail);
                    let alive = t.alive_ticks as f64;
                    BuildRow {
                        build: build.name.clone(),
                        rounds: t.rounds,
                        win_rate: last_standing
                            .then(|| ratio(f64::from(t.wins), f64::from(t.rounds)))
                            .flatten(),
                        kills_per_min: ratio(f64::from(t.kills), alive / TICK_HZ / 60.0),
                        deaths_wall: t.wall,
                        deaths_own_trail: t.own_trail,
                        deaths_trail: t.trail,
                        suicide_share: ratio(f64::from(t.wall + t.own_trail), deaths),
                        median_life_ticks: median(&t.lifetimes),
                        idle_share: ratio(t.idle_ticks as f64, alive),
                        rubber_share: ratio(t.rubber_ticks as f64, alive),
                        mean_speed: ratio(t.speed_sum, alive),
                    }
                })
                .collect();
            SetupReport {
                name: spec.name.clone(),
                riders: spec.riders,
                matches: matches[i],
                rounds_played: rounds_played[i],
                timeouts: timeouts[i],
                draws: draws[i],
                last_standing,
                rows,
                builds: suite.builds.iter().map(|b| b.name.clone()).collect(),
                kills: kills[i].clone(),
            }
        })
        .collect();

    let timing = Timing {
        threads: pool.current_num_threads(),
        builds: suite
            .builds
            .iter()
            .zip(&mut nanos)
            .map(|(build, ns)| {
                ns.sort_unstable();
                let sum: f64 = ns.iter().map(|&n| f64::from(n)).sum();
                BuildTiming {
                    build: build.name.clone(),
                    decisions: ns.len(),
                    mean_us: ratio(sum, ns.len() as f64).unwrap_or(0.0) / 1000.0,
                    p99_us: if ns.is_empty() {
                        0.0
                    } else {
                        quantile(ns, 0.99) / 1000.0
                    },
                }
            })
            .collect(),
    };
    Ok(Report {
        seeds: suite.seeds,
        setups,
        timing,
    })
}

fn pct(value: Option<f64>) -> String {
    value.map_or("-".into(), |v| format!("{:.1}%", v * 100.0))
}

fn num(value: Option<f64>, decimals: usize) -> String {
    value.map_or("-".into(), |v| format!("{v:.decimals$}"))
}

impl Report {
    /// The report as markdown tables, one section per setup, then the cost
    /// of decisions in a section of its own.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        for setup in &self.setups {
            let rounds = if setup.last_standing {
                "rounds"
            } else {
                "matches"
            };
            out += &format!(
                "## {} ({} riders, {} matches, {} seeds)\n\n",
                setup.name, setup.riders, setup.matches, self.seeds
            );
            if setup.last_standing {
                out += &format!(
                    "{} rounds: {} ended on the time limit, {} as draws.\n\n",
                    setup.rounds_played, setup.timeouts, setup.draws
                );
            }
            out += &format!(
                "| build | {rounds} | win | kills/min | wall | own trail | other trail | suicide | median life | idle | rubber | speed |\n"
            );
            out += "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n";
            for r in &setup.rows {
                out += &format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    r.build,
                    r.rounds,
                    pct(r.win_rate),
                    num(r.kills_per_min, 2),
                    r.deaths_wall,
                    r.deaths_own_trail,
                    r.deaths_trail,
                    pct(r.suicide_share),
                    num(r.median_life_ticks, 0),
                    pct(r.idle_share),
                    pct(r.rubber_share),
                    num(r.mean_speed, 3),
                );
            }
            out += "\nKills (row killed column):\n\n| |";
            for name in &setup.builds {
                out += &format!(" {name} |");
            }
            out += "\n|---|";
            out += &"---:|".repeat(setup.builds.len());
            out += "\n";
            for (name, row) in setup.builds.iter().zip(&setup.kills) {
                out += &format!("| {name} |");
                for n in row {
                    out += &format!(" {n} |");
                }
                out += "\n";
            }
            out += "\n";
        }
        out += &format!(
            "## Decision cost (wall clock, NOT deterministic, {} threads)\n\n",
            self.timing.threads
        );
        out += "| build | decisions | mean us | p99 us |\n|---|---:|---:|---:|\n";
        for t in &self.timing.builds {
            out += &format!(
                "| {} | {} | {:.1} | {:.1} |\n",
                t.build, t.decisions, t.mean_us, t.p99_us
            );
        }
        out
    }

    /// JSON of the deterministic part only, for comparing runs.
    pub fn deterministic_json(&self) -> String {
        serde_json::to_string(&self.setups).expect("report serializes")
    }
}
