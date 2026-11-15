//! Every step of the shipped tutorial is solvable and not trivial (idling and
//! its near misses do not pass); steps with
//! other riders play the same in every run, as the hosted server plays them
//! (`crates/scene/fixtures/riders.toml`, also run by `server::tutorial`).

use std::collections::BTreeMap;

use scene::step::{BUILTIN, Fail, parse};
use tron_zero_lab::tutorial::{play, scenario};
use tron_zero_lab::{EventKind, Lab};

const RIDERS: &str = include_str!("../../scene/fixtures/riders.toml");

#[test]
fn every_shipped_step_passes_with_its_solution_and_not_idle() {
    for (i, step) in parse(BUILTIN).unwrap().iter().enumerate() {
        let n = i + 1;
        assert_eq!(play(step, &step.solution), Ok(()), "step {n}: the solution");
        assert!(play(step, &BTreeMap::new()).is_err(), "step {n}: idling");
        for (k, script) in step.must_fail.iter().enumerate() {
            assert!(play(step, script).is_err(), "step {n}: near miss {}", k + 1);
        }
    }
}

#[test]
fn scene_riders_play_the_same_in_every_run() {
    let steps = parse(RIDERS).unwrap();
    for (i, step) in steps.iter().enumerate() {
        let run = || {
            let mut lab = Lab::new(&scenario(step, &BTreeMap::new()));
            lab.run(step.timeout, 30);
            lab.events().to_vec()
        };
        assert_eq!(run(), run(), "step {}", i + 1);
    }
    // The scripted turn lands on placement tick 60, as on the server.
    let mut lab = Lab::new(&scenario(&steps[0], &BTreeMap::new()));
    lab.run(steps[0].timeout, 0);
    let turns: Vec<u32> = lab
        .events()
        .iter()
        .filter(|e| e.rider == "guide" && matches!(e.kind, EventKind::Turn { .. }))
        .map(|e| e.tick)
        .collect();
    assert_eq!(turns, [60]);
    let idle = BTreeMap::new();
    assert_eq!(play(&steps[1], &idle), Ok(()), "a kill on the laid trail");
    assert_eq!(
        play(&steps[2], &idle),
        Err(Fail::RiderDead("victim".into()))
    );
}
