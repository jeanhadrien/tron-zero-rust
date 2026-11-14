//! Tune and ladder do not depend on the thread count; rosters round-trip.
//! Tiny configs: the numbers mean nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use tron_zero_lab::builds::{self, BuildSpec};
use tron_zero_lab::ladder::Ladder;
use tron_zero_lab::tune::Tune;

const SETUP: &str = r#"
[[pool]]
name = "P1"
bot = "adaptive"
[[pool]]
name = "P2"
bot = "survivalist"
difficulty = 1000

[[setup]]
name = "duel"
riders = 2
[setup.match]
respawn = "never"
rounds = 1
[setup.match.end]
last_standing = true
time_limit = 300
"#;

fn tune() -> Tune {
    let text = format!(
        r#"
seeds = 1

[base]
name = "T"
bot = "duelist"

[[param]]
name = "pressure"
min = 0.0
max = 4.0
[[param]]
name = "horizon_ticks"
min = 20
max = 60

[fitness]
win_rate = 1.0
idle_share = -0.5

[sweep]
steps = 2
{SETUP}"#
    );
    Tune::parse(&text, Path::new("")).unwrap()
}

#[test]
fn sweep_is_the_same_on_one_and_many_threads() {
    let tune = tune();
    let one = serde_json::to_string(&tune.sweep(Some(1)).unwrap()).unwrap();
    let many = serde_json::to_string(&tune.sweep(Some(4)).unwrap()).unwrap();
    assert_eq!(one, many);
}

#[test]
fn ladder_is_the_same_on_one_and_many_threads_and_its_roster_round_trips() {
    let text = format!(
        r#"
seeds = 1
check_seeds = 1
rungs = 2
candidates = 3
name = "Rung"

[base]
name = "Ace"
bot = "trapper"
difficulty = 3000
{SETUP}"#
    );
    let ladder = Ladder::parse(&text, Path::new("")).unwrap();
    let one = ladder.run(Some(1)).unwrap();
    let many = ladder.run(Some(4)).unwrap();
    let roster = one.roster().unwrap();
    assert_eq!(roster, many.roster().unwrap());

    let loaded = bots::roster::parse(&roster).unwrap();
    assert_eq!(loaded.len(), 2);
    for (rung, loaded) in one.rungs.iter().zip(&loaded) {
        assert_eq!(rung.build.name, loaded.name);
        assert_eq!(rung.build.rating_id(), loaded.rating_id());
    }
}

#[test]
fn a_tuned_weight_changes_the_build_and_survives_a_roster() {
    let spec: BuildSpec = toml::from_str(
        r#"
name = "W"
bot = "trapper"
[knobs]
noise = 0.05
[weights]
pressure = 3.5
"#,
    )
    .unwrap();
    let build = spec.build().unwrap();
    assert_eq!(build.weights.pressure, 3.5);
    assert_eq!(build.knobs.noise, 0.05);
    let plain = BuildSpec {
        weights: toml::Table::new(),
        ..spec.clone()
    }
    .build()
    .unwrap();
    assert_ne!(build.rating_id(), plain.rating_id());

    let file = builds::roster_file(std::slice::from_ref(&build), "test").unwrap();
    let back = &bots::roster::parse(&file).unwrap()[0];
    assert_eq!(back.rating_id(), build.rating_id());

    // Unknown names fail loudly instead of being ignored.
    let bad: BuildSpec =
        toml::from_str("name = \"B\"\nbot = \"duelist\"\n[weights]\nnope = 1.0").unwrap();
    assert!(bad.build().is_err());
}
