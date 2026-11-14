//! Gates on bot quality: puzzles a good bot must solve, and a small tournament
//! whose self-inflicted deaths must stay rare and fall as difficulty rises.
//!
//! The numbers are measured with margin; when a bot change moves them, check
//! the full suites in `crates/lab/suites/` before touching a bound.

// Test crate: unwraps are fine here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use tron_zero_lab::tournament::{self, Suite};
use tron_zero_lab::{Lab, Scenario, trace};

fn puzzle(name: &str, difficulty: u32, seed: u64) -> Lab {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("puzzles")
        .join(format!("{name}.toml"));
    let mut scenario = Scenario::load(&path).unwrap();
    scenario.riders[0].difficulty = difficulty;
    let mut lab = Lab::with_seed(&scenario, seed);
    lab.run(scenario.until, 0);
    lab
}

/// Strong builds solve every puzzle, whatever the seed.
#[test]
fn strong_bots_solve_the_puzzles() {
    for difficulty in [2500, 4000] {
        for seed in 1..=3 {
            for name in ["box", "dead_end", "narrow_gap"] {
                // The box is the hardest spot: D2500 fills it about 5 times
                // in 6 over 30 seeds (D4000: always), so only D4000 is gated.
                if name == "box" && difficulty < 4000 {
                    continue;
                }
                let lab = puzzle(name, difficulty, seed);
                let bot = lab.rider("bot").unwrap();
                assert!(bot.alive, "{name} D{difficulty} seed {seed}: died");
                if name == "narrow_gap" {
                    // Out of the strip, through the gap.
                    let p = bot.position;
                    let inside = (-200.0..=-100.0).contains(&p.x) && p.y.abs() <= 150.0;
                    assert!(!inside, "{name} D{difficulty} seed {seed}: still inside");
                }
            }
        }
    }
}

#[test]
fn the_same_seed_plays_the_same_game() {
    let scenario = Scenario::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scenarios")
            .join("bot_duel.toml"),
    )
    .unwrap();
    assert_eq!(trace(&scenario, 60), trace(&scenario, 60));
}

/// A hunting puzzle: the rider is `prey`, the bot is `bot`. `personality`
/// replaces the bot's own. Returns how the prey died and whether the bot lives.
fn hunt(name: &str, personality: &str, difficulty: u32, seed: u64) -> (Option<String>, bool) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("puzzles")
        .join(format!("{name}.toml"));
    let mut scenario = Scenario::load(&path).unwrap();
    let bot = scenario
        .riders
        .iter_mut()
        .find(|r| r.name == "bot")
        .unwrap();
    bot.bot = Some(personality.into());
    bot.difficulty = difficulty;
    let mut lab = Lab::with_seed(&scenario, seed);
    lab.run(scenario.until, 0);
    let cause = lab
        .scores()
        .into_iter()
        .find(|s| s.name == "prey")
        .and_then(|s| s.deaths.into_keys().next());
    (cause, lab.rider("bot").unwrap().alive)
}

/// A bot that wants to fight cuts across the path of a rider that never turns;
/// one without that will does not.
#[test]
fn fighters_cut_off_a_rider_and_passive_bots_do_not() {
    for difficulty in [2500, 4000] {
        let (cause, alive) = hunt("intercept", "duelist", difficulty, 1);
        assert_eq!(cause.as_deref(), Some("trail:bot"), "D{difficulty}");
        assert!(alive, "D{difficulty}: the bot died");
    }
    let (cause, _) = hunt("intercept", "passive", 4000, 1);
    assert_ne!(cause.as_deref(), Some("trail:bot"));
}

/// Without a speed advantage the bot cannot catch the rider: it has to grind the
/// rider's trail first, then pass it and cut it off. A passive bot never builds
/// the speed.
#[test]
fn a_bot_builds_speed_to_run_a_rider_down() {
    // Only the duelist at its best does it; the other personalities and levels
    // do not hunt down a rider that fast (see the fight suite).
    for seed in 1..=3 {
        let (cause, alive) = hunt("hunt", "duelist", 4000, seed);
        assert_eq!(cause.as_deref(), Some("trail:bot"), "seed {seed}");
        assert!(alive, "seed {seed}: the bot died");
    }
    let (cause, _) = hunt("hunt", "passive", 4000, 1);
    assert_ne!(cause.as_deref(), Some("trail:bot"));
}

const SUITE: &str = r#"
seeds = 3
default_roster = true

[[build]]
name = "weak"
bot = "adaptive"
difficulty = 500

[[build]]
name = "strong"
bot = "adaptive"
difficulty = 4000

[[setup]]
name = "ffa4"
riders = 4
[setup.match]
respawn = { after = 240 }
[setup.match.end]
time_limit = 1800
"#;

/// Self-inflicted deaths (wall or own trail) per rider-match, by build.
#[test]
fn bots_rarely_kill_themselves_and_better_bots_less() {
    let suite = Suite::parse(SUITE).unwrap();
    let report = tournament::run(&suite, None).unwrap();
    let rows = &report.setups[0].rows;
    let rate = |name: &str| {
        let row = rows.iter().find(|r| r.build == name).unwrap();
        f64::from(row.deaths_wall + row.deaths_own_trail) / f64::from(row.rounds)
    };
    for row in rows {
        if row.build != "weak" {
            assert!(
                rate(&row.build) <= 0.15,
                "{}: {}",
                row.build,
                rate(&row.build)
            );
        }
    }
    assert!(
        rate("strong") <= rate("weak"),
        "{} vs {}",
        rate("strong"),
        rate("weak")
    );
    // The bots grind: a strong build rides clearly above base speed.
    let strong = rows.iter().find(|r| r.build == "strong").unwrap();
    assert!(strong.mean_speed.unwrap() >= 1.1, "{:?}", strong.mean_speed);
}

const ROUNDS_SUITE: &str = r#"
seeds = 4
default_roster = true

[[setup]]
name = "rounds4"
riders = 4
[setup.match]
respawn = "never"
rounds = 3
[setup.match.end]
last_standing = true
time_limit = 3600
"#;

/// Rounds of last rider standing come to an end (the roster measures about 80%
/// ending with a winner), and the riders stay in company and above base speed.
#[test]
fn rounds_end_with_a_winner_and_riders_stay_in_company() {
    let suite = Suite::parse(ROUNDS_SUITE).unwrap();
    let report = tournament::run(&suite, None).unwrap();
    let setup = &report.setups[0];
    let ended = f64::from(setup.rounds_played - setup.timeouts) / f64::from(setup.rounds_played);
    assert!(ended >= 0.55, "only {ended} of the rounds ended");
    let mean = |pick: fn(&tournament::BuildRow) -> Option<f64>| {
        setup.rows.iter().filter_map(pick).sum::<f64>() / setup.rows.len() as f64
    };
    assert!(
        mean(|r| r.idle_share) <= 0.2,
        "idle {}",
        mean(|r| r.idle_share)
    );
    assert!(
        mean(|r| r.mean_speed) >= 1.05,
        "speed {}",
        mean(|r| r.mean_speed)
    );
}
