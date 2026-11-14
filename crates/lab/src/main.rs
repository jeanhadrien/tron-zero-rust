//! `lab <scenario.toml> [options]` — run a scenario headless.
//! `lab tournament <suite.toml> [--quick] [--threads N] [--json PATH] [--no-noise]` — see
//! `tron_zero_lab::tournament`.
//!
//! Options:
//!   --until N         stop at tick N (default: the scenario's `until`)
//!   --seed N          override the scenario's spawn seed
//!   --sample N        add a `sample` event per rider every N ticks
//!   --events PATH     write the JSONL trace to PATH (default: stdout)
//!   --seeds N         batch mode: run seeds S..S+N (S = --seed or the scenario's)
//!                     and write only the rated eliminations, as JSONL, to
//!                     --eliminations PATH (default: stdout)
//!   --eliminations PATH  with --seeds: where the eliminations go
//!   --png PATH        render the final state
//!   --frames DIR      also render DIR/tick-NNNNN.png every `--every` ticks
//!   --every N         frame interval for --frames (default 60)
//!   --view X,Y,SIZE   square world window to render (default: whole arena)
//!   --px N            image width/height in pixels (default 800)
//!
//! The trace and a `#`-prefixed summary go to stdout as program output; this
//! binary does not log.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use glam::Vec2;
use tron_zero_lab::ladder::Ladder;
use tron_zero_lab::tournament::{self, Suite};
use tron_zero_lab::tune::Tune;
use tron_zero_lab::{Lab, PALETTE, Scenario, View, render};

struct Args {
    scenario: PathBuf,
    until: Option<u32>,
    seed: Option<u64>,
    sample: u32,
    events: Option<PathBuf>,
    seeds: Option<u64>,
    eliminations: Option<PathBuf>,
    png: Option<PathBuf>,
    frames: Option<PathBuf>,
    every: u32,
    view: Option<View>,
    px: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let mut args = Args {
        scenario: PathBuf::new(),
        until: None,
        seed: None,
        sample: 0,
        events: None,
        seeds: None,
        eliminations: None,
        png: None,
        frames: None,
        every: 60,
        view: None,
        px: 800,
    };
    let number = |flag: &str, value: Option<String>| -> Result<u32, String> {
        value
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("{flag} expects a number"))
    };
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--until" => args.until = Some(number("--until", raw.next())?),
            "--seed" => {
                let value = raw.next().and_then(|v| v.parse().ok());
                args.seed = Some(value.ok_or("--seed expects a number")?);
            }
            "--sample" => args.sample = number("--sample", raw.next())?,
            "--every" => args.every = number("--every", raw.next())?.max(1),
            "--px" => args.px = number("--px", raw.next())?.max(16),
            "--events" => args.events = raw.next().map(PathBuf::from),
            "--seeds" => {
                let value = raw.next().and_then(|v| v.parse().ok());
                args.seeds = Some(value.ok_or("--seeds expects a number")?);
            }
            "--eliminations" => args.eliminations = raw.next().map(PathBuf::from),
            "--png" => args.png = raw.next().map(PathBuf::from),
            "--frames" => args.frames = raw.next().map(PathBuf::from),
            "--view" => {
                let value = raw.next().unwrap_or_default();
                let parts: Vec<f32> = value.split(',').filter_map(|v| v.parse().ok()).collect();
                let [x, y, size] = parts[..] else {
                    return Err("--view expects X,Y,SIZE".into());
                };
                args.view = Some(View {
                    center: Vec2::new(x, y),
                    size,
                });
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path if args.scenario.as_os_str().is_empty() => args.scenario = path.into(),
            extra => return Err(format!("unexpected argument {extra:?}")),
        }
    }
    if args.scenario.as_os_str().is_empty() {
        return Err(
            "usage: lab <scenario.toml> [--until N] [--seed N] [--sample N] [--events PATH] \
                    [--seeds N --eliminations PATH] [--png PATH] [--frames DIR --every N] [--view X,Y,SIZE] [--px N]"
                .into(),
        );
    }
    Ok(args)
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
fn run_batch(args: &Args, scenario: &Scenario, count: u64) -> Result<(), String> {
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

fn run(args: Args) -> Result<(), String> {
    let scenario = Scenario::load(&args.scenario)?;
    if let Some(count) = args.seeds {
        return run_batch(&args, &scenario, count);
    }
    let until = args.until.unwrap_or(scenario.until);
    let mut lab = Lab::with_seed(&scenario, args.seed.unwrap_or(scenario.seed));
    let view = args.view.unwrap_or_else(|| View::arena(&lab));

    while lab.tick() < until && !lab.done() {
        if let Some(dir) = &args.frames
            && lab.tick().is_multiple_of(args.every)
        {
            save(
                &lab,
                view,
                args.px,
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
        save(&lab, view, args.px, path)?;
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

/// `lab tournament <suite.toml> [--quick] [--threads N] [--json PATH] [--no-noise]`:
/// play every build of a suite and print the result tables as markdown.
fn run_tournament(mut raw: impl Iterator<Item = String>) -> Result<(), String> {
    let mut suite_path: Option<PathBuf> = None;
    let mut threads: Option<usize> = None;
    let mut json: Option<PathBuf> = None;
    let mut no_noise = false;
    let mut quick = false;
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--threads" => {
                let value = raw.next().and_then(|v| v.parse().ok());
                threads = Some(value.ok_or("--threads expects a number")?);
            }
            "--json" => json = Some(raw.next().ok_or("--json expects a path")?.into()),
            "--no-noise" => no_noise = true,
            "--quick" => quick = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path if suite_path.is_none() => suite_path = Some(path.into()),
            extra => return Err(format!("unexpected argument {extra:?}")),
        }
    }
    let usage =
        "usage: lab tournament <suite.toml> [--quick] [--threads N] [--json PATH] [--no-noise]";
    let mut suite = Suite::load(&suite_path.ok_or(usage)?)?;
    if quick {
        suite.quick();
    }
    if no_noise {
        suite.without_noise();
    }
    let report = tournament::run(&suite, threads)?;
    if let Some(path) = json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    io::stdout()
        .lock()
        .write_all(report.markdown().as_bytes())
        .map_err(|e| e.to_string())
}

/// Flags shared by `tune` and `ladder`: one file, `--threads`, `--json`,
/// `--out`, and `--sweep` where the caller allows it.
struct SearchArgs {
    file: PathBuf,
    threads: Option<usize>,
    json: Option<PathBuf>,
    out: Option<PathBuf>,
    sweep: bool,
}

fn search_args(
    mut raw: impl Iterator<Item = String>,
    usage: &str,
    sweep_allowed: bool,
) -> Result<SearchArgs, String> {
    let mut args = SearchArgs {
        file: PathBuf::new(),
        threads: None,
        json: None,
        out: None,
        sweep: false,
    };
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--threads" => {
                let value = raw.next().and_then(|v| v.parse().ok());
                args.threads = Some(value.ok_or("--threads expects a number")?);
            }
            "--json" => args.json = Some(raw.next().ok_or("--json expects a path")?.into()),
            "--out" => args.out = Some(raw.next().ok_or("--out expects a path")?.into()),
            "--sweep" if sweep_allowed => args.sweep = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path if args.file.as_os_str().is_empty() => args.file = path.into(),
            extra => return Err(format!("unexpected argument {extra:?}")),
        }
    }
    if args.file.as_os_str().is_empty() {
        return Err(usage.into());
    }
    Ok(args)
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

/// `lab tune <tune.toml> [--sweep] [--threads N] [--json PATH] [--out PATH]`:
/// search the parameters of a build (see `tron_zero_lab::tune`). `--out`
/// receives the best build as a roster entry.
fn run_tune(raw: impl Iterator<Item = String>) -> Result<(), String> {
    let usage = "usage: lab tune <tune.toml> [--sweep] [--threads N] [--json PATH] [--out PATH]";
    let args = search_args(raw, usage, true)?;
    let tune = Tune::load(&args.file)?;
    if args.sweep {
        let report = tune.sweep(args.threads)?;
        if let Some(path) = &args.json {
            let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            write(path, &text)?;
        }
        return print(&report.markdown());
    }
    let report = tune.search(args.threads)?;
    if let Some(path) = &args.json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        write(path, &text)?;
    }
    if let Some(path) = &args.out {
        write(path, &report.build)?;
    }
    print(&report.markdown())
}

/// `lab ladder <ladder.toml> [--threads N] [--json PATH] [--out PATH]`:
/// build a difficulty ladder (see `tron_zero_lab::ladder`). `--out` receives
/// the roster file; without it the roster is printed after the report.
fn run_ladder(raw: impl Iterator<Item = String>) -> Result<(), String> {
    let usage = "usage: lab ladder <ladder.toml> [--threads N] [--json PATH] [--out PATH]";
    let args = search_args(raw, usage, false)?;
    let report = Ladder::load(&args.file)?.run(args.threads)?;
    let roster = report.roster()?;
    if let Some(path) = &args.json {
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
    let mut raw = std::env::args().skip(1).peekable();
    let result = match raw.peek().map(String::as_str) {
        Some("tournament") => {
            raw.next();
            run_tournament(raw)
        }
        Some("tune") => {
            raw.next();
            run_tune(raw)
        }
        Some("ladder") => {
            raw.next();
            run_ladder(raw)
        }
        _ => parse_args().and_then(run),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(io::stderr(), "lab: {message}");
            ExitCode::FAILURE
        }
    }
}
