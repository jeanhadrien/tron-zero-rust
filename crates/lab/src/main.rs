//! `lab <scenario.toml> [options]` — run a scenario headless.
//!
//! Options:
//!   --until N         stop at tick N (default: the scenario's `until`)
//!   --seed N          override the scenario's spawn seed
//!   --sample N        add a `sample` event per rider every N ticks
//!   --events PATH     write the JSONL trace to PATH (default: stdout)
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
use tron_zero_lab::{Lab, PALETTE, Scenario, View, render};

struct Args {
    scenario: PathBuf,
    until: Option<u32>,
    seed: Option<u64>,
    sample: u32,
    events: Option<PathBuf>,
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
                    [--png PATH] [--frames DIR --every N] [--view X,Y,SIZE] [--px N]"
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

fn run(args: Args) -> Result<(), String> {
    let scenario = Scenario::load(&args.scenario)?;
    let until = args.until.unwrap_or(scenario.until);
    let mut lab = Lab::with_seed(&scenario, args.seed.unwrap_or(scenario.seed));
    let view = args.view.unwrap_or_else(|| View::arena(&lab));

    while lab.tick() < until {
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
        let trail = shared::Trail(rider.trail).length() + 0.0;
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
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(io::stderr(), "lab: {message}");
            ExitCode::FAILURE
        }
    }
}
