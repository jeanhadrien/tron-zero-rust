//! The `lab` command: run a scenario headless, or play suites, sweeps and
//! ladders of bot builds. `lab --help` and `lab <command> --help` are the
//! usage; the formats are in `docs/guides/dev.md` and the modules of
//! `tron_zero_lab`.
//!
//! The trace and a `#`-prefixed summary go to stdout as program output; this
//! binary does not log.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use glam::Vec2;
use tron_zero_lab::ladder::Ladder;
use tron_zero_lab::tournament::{self, Suite};
use tron_zero_lab::tune::Tune;
use tron_zero_lab::{Lab, PALETTE, Scenario, View, render};

#[derive(Parser)]
#[command(
    name = "lab",
    version,
    about = "Headless, deterministic runs of the shared sim: a scenario (trace + PNG), or tournaments, sweeps and ladders of bot builds",
    // `lab <scenario.toml> [options]` is the default command.
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    subcommand_precedence_over_arg = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: Option<RunArgs>,
}

#[derive(Subcommand)]
enum Command {
    /// Play every build of a suite and print the result tables as markdown
    Tournament(TournamentArgs),
    /// Sweep the knobs and weights of a build, one parameter at a time
    Tune(TuneArgs),
    /// Build a difficulty ladder and write it as a roster file
    Ladder(LadderArgs),
}

/// Run a scenario: the JSONL trace and a summary go to stdout.
#[derive(Args)]
struct RunArgs {
    /// Scenario file (TOML)
    scenario: PathBuf,
    /// Stop at tick N (default: the scenario's `until`)
    #[arg(long, value_name = "N")]
    until: Option<u32>,
    /// Override the scenario's spawn seed
    #[arg(long, value_name = "N")]
    seed: Option<u64>,
    /// Add a `sample` event per rider every N ticks
    #[arg(long, value_name = "N", default_value_t = 0)]
    sample: u32,
    /// Write the JSONL trace to PATH (default: stdout)
    #[arg(long, value_name = "PATH")]
    events: Option<PathBuf>,
    /// Batch mode: run seeds S..S+N (S = --seed or the scenario's) and write
    /// only the rated eliminations, as JSONL
    #[arg(long, value_name = "N")]
    seeds: Option<u64>,
    /// With --seeds: where the eliminations go (default: stdout)
    #[arg(long, value_name = "PATH")]
    eliminations: Option<PathBuf>,
    /// Render the final state
    #[arg(long, value_name = "PATH")]
    png: Option<PathBuf>,
    /// Also render DIR/tick-NNNNN.png every --every ticks
    #[arg(long, value_name = "DIR")]
    frames: Option<PathBuf>,
    /// Frame interval for --frames
    #[arg(long, value_name = "N", default_value_t = 60)]
    every: u32,
    /// Square world window to render (default: the whole arena)
    #[arg(long, value_name = "X,Y,SIZE", value_parser = parse_view, allow_hyphen_values = true)]
    view: Option<View>,
    /// Image width and height in pixels
    #[arg(long, value_name = "N", default_value_t = 800)]
    px: u32,
}

fn parse_view(text: &str) -> Result<View, String> {
    let parts: Vec<f32> = text.split(',').filter_map(|v| v.parse().ok()).collect();
    let [x, y, size] = parts[..] else {
        return Err("expected X,Y,SIZE".into());
    };
    Ok(View {
        center: Vec2::new(x, y),
        size,
    })
}

#[derive(Args)]
struct TournamentArgs {
    /// Suite file (TOML, see `crates/lab/suites/`)
    suite: PathBuf,
    /// Cut the seed count to the suite's `quick_seeds`: the normal way to measure
    #[arg(long)]
    quick: bool,
    /// Play every build with `noise = 0`
    #[arg(long)]
    no_noise: bool,
    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Args)]
struct TuneArgs {
    /// Tune file (TOML, see `crates/lab/tuning/`)
    file: PathBuf,
    #[command(flatten)]
    common: CommonArgs,
    /// Write the best single-parameter build as a roster entry
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct LadderArgs {
    /// Ladder file (TOML, see `crates/lab/tuning/`)
    file: PathBuf,
    #[command(flatten)]
    common: CommonArgs,
    /// Write the roster file to PATH (default: print it after the report)
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
}

/// Flags of the commands that play many matches.
#[derive(Args)]
struct CommonArgs {
    /// Worker threads (default: half the cores)
    #[arg(long, value_name = "N")]
    threads: Option<usize>,
    /// Also write the report as JSON
    #[arg(long, value_name = "PATH")]
    json: Option<PathBuf>,
}

fn save(lab: &Lab, view: View, px: u32, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    render(lab, view, px)
        .save_png(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Batch mode: run `count` consecutive seeds and write only the rated
/// eliminations. Seeds are independent matches; a rating replay consumes the
/// lines in order.
fn run_batch(args: &RunArgs, scenario: &Scenario, count: u64) -> Result<(), String> {
    let until = args.until.unwrap_or(scenario.until);
    let first = args.seed.unwrap_or(scenario.seed);
    let mut out = String::new();
    for seed in first..first + count {
        let mut lab = Lab::with_seed(scenario, seed);
        lab.run(until, 0);
        out.push_str(&tron_zero_lab::eliminations_jsonl(lab.eliminations()));
    }
    match &args.eliminations {
        Some(path) => std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display())),
        None => io::stdout()
            .lock()
            .write_all(out.as_bytes())
            .map_err(|e| e.to_string()),
    }
}

fn run(args: RunArgs) -> Result<(), String> {
    let scenario = Scenario::load(&args.scenario)?;
    if let Some(count) = args.seeds {
        return run_batch(&args, &scenario, count);
    }
    let until = args.until.unwrap_or(scenario.until);
    let mut lab = Lab::with_seed(&scenario, args.seed.unwrap_or(scenario.seed));
    let view = args.view.unwrap_or_else(|| View::arena(&lab));

    while lab.tick() < until && !lab.done() {
        if let Some(dir) = &args.frames
            && lab.tick().is_multiple_of(args.every.max(1))
        {
            save(
                &lab,
                view,
                args.px.max(16),
                &dir.join(format!("tick-{:05}.png", lab.tick())),
            )?;
        }
        if args.sample > 0 && lab.tick().is_multiple_of(args.sample) {
            lab.sample();
        }
        lab.step();
    }

    let trace = tron_zero_lab::to_jsonl(lab.events());
    let mut out = io::stdout().lock();
    let io_err = |e: io::Error| e.to_string();
    match &args.events {
        Some(path) => {
            std::fs::write(path, &trace).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => out.write_all(trace.as_bytes()).map_err(io_err)?,
    }
    if let Some(path) = &args.png {
        save(&lab, view, args.px.max(16), path)?;
    }

    writeln!(out, "# tick {until}").map_err(io_err)?;
    for (rider, score) in lab.riders().into_iter().zip(lab.scores()) {
        let color = PALETTE[rider.index % PALETTE.len()].0;
        let state = if rider.alive { "alive" } else { "dead" };
        // `+ 0.0` turns the empty sum's -0.0 into 0.0 for display.
        let trail = shared::polyline::length(&rider.trail) + 0.0;
        writeln!(
            out,
            "# {} ({color}) {state} at ({:.1}, {:.1}) heading {} speed {:.3} target {:.3} \
             rubber {:.1} trail {trail:.1} deaths {} kills {}",
            rider.name,
            rider.position.x,
            rider.position.y,
            tron_zero_lab::scenario::Heading::label(rider.direction),
            rider.speed,
            rider.target,
            rider.rubber,
            rider.deaths.len(),
            score.kills,
        )
        .map_err(io_err)?;
        if !score.deaths.is_empty() {
            let causes: Vec<String> = score
                .deaths
                .iter()
                .map(|(cause, n)| format!("{cause} x{n}"))
                .collect();
            writeln!(out, "#   died on {}", causes.join(", ")).map_err(io_err)?;
        }
        let names: Vec<String> = lab.riders().into_iter().map(|r| r.name).collect();
        let forced: Vec<String> = lab
            .forced()
            .iter()
            .filter(|d| d.victim == rider.index)
            .map(|d| {
                let by = d.by.map_or("nobody", |k| names[k].as_str());
                format!("tick {} forced by {by}", d.tick)
            })
            .collect();
        if !forced.is_empty() {
            writeln!(out, "#   own-trail/wall deaths: {}", forced.join(", ")).map_err(io_err)?;
        }
    }
    Ok(())
}

fn run_tournament(args: TournamentArgs) -> Result<(), String> {
    let mut suite = Suite::load(&args.suite)?;
    if args.quick {
        suite.quick();
    }
    if args.no_noise {
        suite.without_noise();
    }
    let report = tournament::run(&suite, args.common.threads)?;
    if let Some(path) = &args.common.json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        write(path, &text)?;
    }
    print(&report.markdown())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn print(text: &str) -> Result<(), String> {
    io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())
}

fn run_tune(args: TuneArgs) -> Result<(), String> {
    let tune = Tune::load(&args.file)?;
    let report = tune.sweep(args.common.threads)?;
    if let Some(path) = &args.common.json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        write(path, &text)?;
    }
    if let Some(path) = &args.out {
        write(path, &tune.best_entry(&report)?)?;
    }
    print(&report.markdown())
}

fn run_ladder(args: LadderArgs) -> Result<(), String> {
    let report = Ladder::load(&args.file)?.run(args.common.threads)?;
    let roster = report.roster()?;
    if let Some(path) = &args.common.json {
        let text = serde_json::to_string_pretty(&report.json()).map_err(|e| e.to_string())?;
        write(path, &text)?;
    }
    print(&report.markdown())?;
    match &args.out {
        Some(path) => write(path, &roster),
        None => print(&format!(
            "
```toml
{roster}```
"
        )),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Tournament(args)) => run_tournament(args),
        Some(Command::Tune(args)) => run_tune(args),
        Some(Command::Ladder(args)) => run_ladder(args),
        // Without a command clap has already required the scenario.
        None => cli.run.map_or(Ok(()), run),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(io::stderr(), "lab: {message}");
            ExitCode::FAILURE
        }
    }
}
