//! The tournament report must not depend on how many threads played it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use tron_zero_lab::tournament::{self, Suite};

const SUITE: &str = r#"
seeds = 2
default_roster = true

[[setup]]
name = "duel"
riders = 2
[setup.match]
respawn = { after = 60 }
[setup.match.end]
time_limit = 600

[[setup]]
name = "rounds"
riders = 3
[setup.match]
rounds = 2
[setup.match.end]
last_standing = true
time_limit = 600
"#;

#[test]
fn report_is_identical_for_one_and_many_threads() {
    let suite = Suite::parse(SUITE).unwrap();
    let one = tournament::run(&suite, Some(1)).unwrap();
    let many = tournament::run(&suite, Some(4)).unwrap();
    assert_eq!(one.deterministic_json(), many.deterministic_json());
    // Duel: C(5,2) pairs x 2 seats x 2 seeds; the rounds setup plays 2 rounds each.
    assert_eq!(one.setups[0].matches, 40);
    assert_eq!(one.setups[1].matches, 60);
    assert!(one.setups[1].rows.iter().all(|r| r.win_rate.is_some()));
    assert!(one.setups[0].rows.iter().all(|r| r.win_rate.is_none()));
}
