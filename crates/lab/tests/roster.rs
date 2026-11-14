//! The committed difficulty ladder (`crates/bots/rosters/ladder.toml`) keeps
//! its promises.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use serde::Deserialize;
use tron_zero_lab::bench::Bench;
use tron_zero_lab::tournament::SetupSpec;

#[derive(Deserialize)]
struct Setups {
    setup: Vec<SetupSpec>,
}

/// Quick size of the ladder check: seeds held out from the ones the ladder
/// was built on (`lab ladder` uses 100000.. for its own check as well).
const SEEDS: u64 = 4;

/// Each rung of the difficulty ladder beats the rung below it in more than
/// half of the decided 1v1 rounds. The ladder is the first five builds; later
/// ones (the artists) are outside it.
#[test]
fn the_ladder_is_monotonic() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bots/rosters/ladder.toml");
    let roster = bots::roster::load(&path).unwrap();
    let rungs = &roster[..5];
    let setups: Setups = toml::from_str(
        r#"
[[setup]]
name = "duel"
riders = 2
[setup.match]
respawn = "never"
rounds = 4
[setup.match.end]
last_standing = true
time_limit = 3600
"#,
    )
    .unwrap();
    let bench = Bench::new(setups.setup, None, rungs.to_vec()).unwrap();
    for pair in rungs.windows(2) {
        let [high, low] = bench.duel(&pair[1], &pair[0], 100_000, SEEDS).unwrap();
        assert!(
            high > low,
            "{} won {high} rounds against {}'s {low}",
            pair[1].name,
            pair[0].name
        );
    }
}
