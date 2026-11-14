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
}
