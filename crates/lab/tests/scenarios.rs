//! Scenario tests: normative mechanics as assertions, plus golden traces for
//! every file in `scenarios/` to catch unintended behaviour drift.
//!
//! Regenerate goldens after an intended change with `LAB_BLESS=1 cargo test
//! -p tron-zero-lab`, then review the diff like code.

// Test crate: unwraps and prints are fine here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

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
    let traces: Vec<(PathBuf, String)> = entries
        .into_iter()
        .map(|path| {
            let actual = trace(&Scenario::load(&path).unwrap(), SAMPLE);
            (path, actual)
        })
        .collect();

    if bless {
        bless_goldens(&traces);
        return;
    }
    let stale: Vec<String> = traces
        .iter()
        .filter(|(path, actual)| {
            let expected = std::fs::read_to_string(path.with_extension("golden.jsonl"))
                .unwrap_or_default()
                .replace("\r\n", "\n");
            *actual != expected
        })
        .map(|(path, _)| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        stale.is_empty(),
        "traces changed for {stale:?}; inspect with `cargo run -p tron-zero-lab -- \
         <scenario> --sample {SAMPLE}` and bless with LAB_BLESS=1 if intended"
    );
}

/// FNV-1a over every trace (name and content), in scenario order.
fn digest(traces: &[(PathBuf, String)]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (path, trace) in traces {
        let name = path.file_name().unwrap().to_string_lossy();
        for byte in name.bytes().chain([0]).chain(trace.bytes()).chain([0]) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Writes the goldens and stamps them with the protocol version they belong
/// to. Traces that changed (the sim now behaves differently) are only blessed
/// once `PROTOCOL_VERSION` was bumped, or when `LAB_BLESS_NO_BUMP=1` says the
/// change cannot affect a peer (new scenario only, trace format, ...).
fn bless_goldens(traces: &[(PathBuf, String)]) {
    let stamp_path = scenarios_dir().join("goldens.protocol");
    let stamp = std::fs::read_to_string(&stamp_path).unwrap_or_default();
    let field = |key: &str| {
        stamp
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let changed_existing = traces.iter().any(|(path, actual)| {
        std::fs::read_to_string(path.with_extension("golden.jsonl"))
            .map(|old| old.replace("\r\n", "\n") != *actual)
            .unwrap_or(false)
    });
    let bumped =
        field("protocol_version = ") != Some(u64::from(shared::protocol::PROTOCOL_VERSION));
    let no_bump = std::env::var("LAB_BLESS_NO_BUMP").is_ok_and(|v| v == "1");
    assert!(
        !changed_existing || bumped || no_bump,
        "existing goldens changed: the sim behaves differently, so bump PROTOCOL_VERSION \
         (crates/shared/src/protocol.rs) before blessing, or set LAB_BLESS_NO_BUMP=1 if no \
         peer could be affected"
    );
    for (path, actual) in traces {
        std::fs::write(path.with_extension("golden.jsonl"), actual).unwrap();
    }
    std::fs::write(
        stamp_path,
        format!(
            "protocol_version = {}\ndigest = {}\n",
            shared::protocol::PROTOCOL_VERSION,
            digest(traces)
        ),
    )
    .unwrap();
}

/// A golden edited or blessed without the stamp (or a stamp left behind by a
/// merge) means the version decision was skipped.
#[test]
fn goldens_are_stamped_with_a_protocol_version() {
    let stamp = std::fs::read_to_string(scenarios_dir().join("goldens.protocol"))
        .expect("goldens.protocol missing: bless with LAB_BLESS=1");
    let goldens: Vec<(PathBuf, String)> = {
        let mut paths: Vec<_> = std::fs::read_dir(scenarios_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let golden = std::fs::read_to_string(path.with_extension("golden.jsonl"))
                    .unwrap()
                    .replace("\r\n", "\n");
                (path, golden)
            })
            .collect()
    };
    assert!(
        stamp.contains(&format!("digest = {}\n", digest(&goldens))),
        "goldens do not match goldens.protocol: re-bless with LAB_BLESS=1"
    );
    assert!(
        stamp.contains(&format!(
            "protocol_version = {}\n",
            shared::protocol::PROTOCOL_VERSION
        )),
        "PROTOCOL_VERSION changed since the goldens were stamped: re-bless with LAB_BLESS=1 \
         (LAB_BLESS_NO_BUMP=1 is not needed, the version already moved)"
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
    // Rubber is paid per unit of distance not travelled, so the zone's soft
    // edge is free and the full drain only arrives at the wall: ~25 ticks in
    // the zone at base speed. The rider must never touch the wall meanwhile.
    let in_zone = died[0] - entered[0];
    assert!((22..=30).contains(&in_zone), "{in_zone} ticks in zone");
    let a = lab.rider("a").unwrap();
    assert!(a.deaths[0].x < 1200.0);
}

#[test]
fn turning_away_escapes_and_rubber_regenerates() {
    let lab = run("escape");
    let a = lab.rider("a").unwrap();
    assert!(a.alive);
    assert!(death_causes(&lab, "a").is_empty());
    // Regen is proportional to what is missing (about 5 s time constant): a
    // good part of it is back, never all of it.
    assert!(
        a.rubber > 60.0 && a.rubber < shared::BASE_RUBBER,
        "rubber {}",
        a.rubber
    );
    // Its escape route hugs the east wall: grinding boosts the target speed.
    assert!(a.target > 1.1, "target {}", a.target);
}

#[test]
fn grinding_boosts_and_open_space_decays_to_base() {
    let scenario = load("grind");
    let mut lab = Lab::new(&scenario);
    lab.run(300, 0);
    let boosted = lab.rider("a").unwrap().target;
    assert!(boosted > 1.2, "boost {boosted}");
    lab.run(scenario.until, 0);
    let a = lab.rider("a").unwrap();
    assert!(a.alive);
    // The boost fades exponentially: close to base, never exactly there.
    assert!(a.target > 1.0 && a.target < 1.05, "target {}", a.target);
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

#[test]
fn grind_speed_rises_with_closeness_and_respects_the_cap() {
    let lab = run("grind_cap");
    let target = |name: &str| lab.rider(name).unwrap().target;
    assert!(target("d2") > target("d4"));
    assert!(target("d4") > target("d6"));
    assert!(target("d6") > target("d9"));
    assert!(target("d9") > target("d12"));
    assert!(target("d12") > 1.0);
    assert_eq!(target("dnone"), 1.0);
    // Walls pay a fraction of the trail boost, even at near contact.
    assert!(target("d2") <= rim_cap());
}

#[test]
fn full_speed_rubber_outlasts_the_late_turn_window() {
    let lab = run("wall_stop_fast");
    let entered = ticks(&lab, "a", |k| {
        matches!(k, EventKind::EnterRubberZone { .. })
    });
    let died = ticks(&lab, "a", |k| matches!(k, EventKind::Death { .. }));
    // A turn arriving up to 12 ticks late (netcode TURN_LATE_WINDOW_TICKS)
    // must still be able to save a cornered rider at the cap. The zone is
    // wide at 3x and its outer ~4 ticks are free, so the late window plus
    // those 4 ticks from entry is about the window's worth of real contact.
    let needed = shared::protocol::TURN_LATE_WINDOW_TICKS + 4;
    assert!(
        died[0] - entered[0] >= needed,
        "{} ticks",
        died[0] - entered[0]
    );
}

/// Terminal speed of one wall touching a rider: what rim riding can reach.
fn rim_cap() -> f32 {
    1.0 + shared::GRIND_ACCEL_PER_SEC / shared::SPEED_DECAY_PER_SEC * shared::ARENA_WALL_GRIND
}

#[test]
fn trails_pay_out_over_the_full_grind_range() {
    let scenario = load("trail_grind");
    let mut lab = Lab::new(&scenario);
    lab.run(200, 0);
    // A trail 12 u away pays more than any wall can, but less than touching.
    let target = lab.rider("a").unwrap().target;
    assert!(target > rim_cap() && target < 2.0, "target {target}");
}

/// Highest cruise speed `rider` reaches up to tick `until` of the scenario.
fn peak_target_until(name: &str, rider: &str, until: u32) -> f32 {
    let scenario = load(name);
    let mut lab = Lab::new(&scenario);
    let mut peak = 1.0_f32;
    for tick in 1..=until {
        lab.run(tick, 0);
        peak = peak.max(lab.rider(rider).unwrap().target);
    }
    peak
}

/// Highest cruise speed `rider` reaches while the scenario plays out.
fn peak_target(name: &str, rider: &str) -> f32 {
    peak_target_until(name, rider, load(name).until)
}

#[test]
fn own_trail_grinds_but_not_the_leg_a_double_tap_uturn_leaves() {
    // Riding back along the outbound leg 3 u away is a free ride in open
    // space: that leg is too close to the head in trail order to pay.
    assert_eq!(peak_target_until("self_grind", "a", 380), 1.0);
    // The second U-turn rides along the outbound leg again, now an older leg.
    assert!(peak_target("self_grind", "a") > 1.2);
}

#[test]
fn edge_riding_pays_far_less_than_trail_grinding() {
    let edge = peak_target("edge_ride", "a");
    assert!(edge > 1.0 && edge <= rim_cap() + 1e-3, "edge {edge}");
    let trail = peak_target("trail_grind", "a");
    assert!(trail > edge + 0.1, "trail {trail} vs edge {edge}");
}

#[test]
fn rounds_end_on_the_last_rider_and_name_the_winner() {
    let lab = run("rounds");
    let scenario = load("rounds");
    let rules = scenario.rules.unwrap();
    assert!(lab.done());
    assert_eq!(lab.rounds().len(), rules.rounds as usize);
    for result in lab.rounds() {
        let dead_in_round: Vec<&str> = lab
            .events()
            .iter()
            .filter(|e| (result.start..=result.end).contains(&e.tick))
            .filter(|e| matches!(e.kind, EventKind::Death { .. }))
            .map(|e| e.rider.as_str())
            .collect();
        let riders = lab.riders();
        let alive: Vec<&str> = riders
            .iter()
            .map(|r| r.name.as_str())
            .filter(|n| !dead_in_round.contains(n))
            .collect();
        match result.winner {
            // Exactly one rider outlived the others.
            Some(winner) => {
                assert_eq!(result.reason, shared::RoundEnd::LastStanding);
                assert_eq!(alive, [riders[winner].name.as_str()]);
            }
            None if result.reason == shared::RoundEnd::TimeLimit => {
                assert_eq!(result.end + 1 - result.start, rules.end.time_limit.unwrap());
                assert!(alive.len() > 1);
            }
            None => assert!(alive.is_empty(), "a draw leaves nobody alive"),
        }
    }
    // Every round starts with everyone alive on a fresh trail.
    let starts = lab
        .events()
        .iter()
        .filter(|e| matches!(e.kind, EventKind::RoundStart { .. }))
        .count();
    assert_eq!(starts, rules.rounds as usize);
}

#[test]
fn a_pinned_tier_overrides_the_riders_alive_count() {
    // Default behaviour (no pin) is covered by the goldens.
    let pinned = |tier| {
        let mut scenario = load("bot_arena");
        scenario.tier = Some(tier);
        trace(&scenario, 0)
    };
    use tron_zero_lab::scenario::TierPin;
    assert_ne!(pinned(TierPin::Full), pinned(TierPin::T2));
}
