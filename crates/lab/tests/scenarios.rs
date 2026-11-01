//! Scenario tests: normative mechanics as assertions, plus golden traces for
//! every file in `scenarios/` to catch unintended behaviour drift.
//!
//! Regenerate goldens after an intended change with `LAB_BLESS=1 cargo test
//! -p tron-zero-lab`, then review the diff like code.

use std::path::{Path, PathBuf};

use tron_zero_lab::{EventKind, Lab, Scenario, trace};

const SAMPLE: u32 = 60;

fn scenarios_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

fn load(name: &str) -> Scenario {
    Scenario::load(&scenarios_dir().join(format!("{name}.toml"))).unwrap()
}

fn run(name: &str) -> Lab {
    let scenario = load(name);
    let mut lab = Lab::new(&scenario);
    lab.run(scenario.until, 0);
    lab
}

/// Ticks of `rider`'s events matching `pred`.
fn ticks(lab: &Lab, rider: &str, pred: impl Fn(&EventKind) -> bool) -> Vec<u32> {
    lab.events()
        .iter()
        .filter(|e| e.rider == rider && pred(&e.kind))
        .map(|e| e.tick)
        .collect()
}

fn death_causes(lab: &Lab, rider: &str) -> Vec<String> {
    lab.events()
        .iter()
        .filter(|e| e.rider == rider)
        .filter_map(|e| match &e.kind {
            EventKind::Death { cause, .. } => Some(cause.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn golden_traces_match() {
    let bless = std::env::var("LAB_BLESS").is_ok_and(|v| v == "1");
    let mut entries: Vec<_> = std::fs::read_dir(scenarios_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty());
    let mut stale = Vec::new();
    for path in entries {
        let actual = trace(&Scenario::load(&path).unwrap(), SAMPLE);
        let golden = path.with_extension("golden.jsonl");
        if bless {
            std::fs::write(&golden, &actual).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&golden)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if actual != expected {
            stale.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(
        stale.is_empty(),
        "traces changed for {stale:?}; inspect with `cargo run -p tron-zero-lab -- \
         <scenario> --sample {SAMPLE}` and bless with LAB_BLESS=1 if intended"
    );
}

#[test]
fn runs_are_deterministic() {
    for name in ["head_on", "respawn", "grind"] {
        let scenario = load(name);
        assert_eq!(trace(&scenario, SAMPLE), trace(&scenario, SAMPLE), "{name}");
    }
}

#[test]
fn facing_a_wall_drains_rubber_then_kills() {
    let lab = run("wall_stop");
    let entered = ticks(&lab, "a", |k| {
        matches!(k, EventKind::EnterRubberZone { .. })
    });
    let died = ticks(&lab, "a", |k| matches!(k, EventKind::Death { .. }));
    assert_eq!(death_causes(&lab, "a"), ["wall"]);
    // Base-speed drain is 12 * 0.03 * 2^3 = 2.88 per tick: 120 rubber lasts
    // ~42 ticks in the zone. The rider must never touch the wall meanwhile.
    let in_zone = died[0] - entered[0];
    assert!((40..=44).contains(&in_zone), "{in_zone} ticks in zone");
    let a = lab.rider("a").unwrap();
    assert!(a.deaths[0].x < 1200.0);
}

#[test]
fn turning_away_escapes_and_rubber_regenerates() {
    let lab = run("escape");
    let a = lab.rider("a").unwrap();
    assert!(a.alive);
    assert!(death_causes(&lab, "a").is_empty());
    assert_eq!(a.rubber, shared::BASE_RUBBER);
    // Its escape route hugs the east wall: grinding boosts the target speed.
    assert!(a.target > 1.2, "target {}", a.target);
}

#[test]
fn grinding_boosts_and_open_space_decays_to_base() {
    let scenario = load("grind");
    let mut lab = Lab::new(&scenario);
    lab.run(300, 0);
    let boosted = lab.rider("a").unwrap().target;
    assert!(boosted > 1.5, "boost {boosted}");
    lab.run(scenario.until, 0);
    let a = lab.rider("a").unwrap();
    assert!(a.alive);
    assert_eq!(a.target, 1.0);
}

#[test]
fn head_on_kills_both_on_each_others_trail() {
    let lab = run("head_on");
    assert_eq!(death_causes(&lab, "a"), ["trail:b"]);
    assert_eq!(death_causes(&lab, "b"), ["trail:a"]);
    let a = ticks(&lab, "a", |k| matches!(k, EventKind::Death { .. }));
    let b = ticks(&lab, "b", |k| matches!(k, EventKind::Death { .. }));
    assert_eq!(a, b, "symmetric riders must die on the same tick");
}

#[test]
fn closing_a_loop_dies_on_own_trail() {
    let lab = run("own_trail");
    assert_eq!(death_causes(&lab, "a"), ["own_trail"]);
}

#[test]
fn respawn_lands_on_a_safe_spawn_and_rides_on() {
    let lab = run("respawn");
    let respawns = ticks(&lab, "a", |k| matches!(k, EventKind::Respawn { .. }));
    assert_eq!(respawns, [200]);
    let a = lab.rider("a").unwrap();
    assert!(a.alive);
    assert_eq!(a.deaths.len(), 1);
    assert_eq!(a.rubber, shared::BASE_RUBBER);
}

#[test]
fn bot_arenas_replay_per_seed_and_vary_across_seeds() {
    let scenario = load("bot_arena");
    let run = |seed| {
        let mut lab = Lab::with_seed(&scenario, seed);
        lab.run(600, SAMPLE);
        tron_zero_lab::to_jsonl(lab.events())
    };
    assert_eq!(run(3), run(3));
    assert_ne!(run(3), run(4));
}

#[test]
fn bots_drive_and_respawn_like_the_server() {
    let lab = run("bot_arena");
    let until = load("bot_arena").until;
    for rider in lab.riders() {
        assert!(rider.is_bot);
        let turns = ticks(&lab, &rider.name, |k| matches!(k, EventKind::Turn { .. }));
        assert!(!turns.is_empty(), "{} never turned", rider.name);
        // Every death respawns 240 ticks later, as the server's bot spawner.
        let deaths = ticks(&lab, &rider.name, |k| matches!(k, EventKind::Death { .. }));
        let respawns = ticks(&lab, &rider.name, |k| {
            matches!(k, EventKind::Respawn { .. })
        });
        let expected: Vec<u32> = deaths
            .iter()
            .map(|t| t + 240)
            .filter(|&t| t < until)
            .collect();
        assert_eq!(respawns, expected, "{}", rider.name);
    }
}
