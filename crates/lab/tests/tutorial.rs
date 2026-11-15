//! Every step of the shipped tutorial is solvable and not trivial.

use std::collections::BTreeMap;

use scene::step::{BUILTIN, parse};
use tron_zero_lab::tutorial::play;

#[test]
fn every_shipped_step_passes_with_its_solution_and_not_idle() {
    for (i, step) in parse(BUILTIN).unwrap().iter().enumerate() {
        let n = i + 1;
        assert_eq!(play(step, &step.solution), Ok(()), "step {n}: the solution");
        assert!(play(step, &BTreeMap::new()).is_err(), "step {n}: idling");
    }
}
