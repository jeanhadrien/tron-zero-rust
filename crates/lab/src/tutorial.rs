//! Play a tutorial step in the lab: the player rides `script`, the step's
//! `scene::step::Judge` decides, as on the hosted server. Checks that a step
//! is solvable (its `solution` passes) and not trivial (idling does not).

use std::collections::BTreeMap;

use scene::Action;
use scene::step::{Death, Fail, Judge, PLAYER, RiderState, Step};

use crate::{EventKind, Lab, Scenario};

/// The attempt's outcome with the player riding `script` (ticks from
/// placement): `Ok` on a pass, the fail otherwise.
pub fn play(step: &Step, script: &BTreeMap<u32, Action>) -> Result<(), Fail> {
    let mut lab = Lab::new(&scenario(step, script));
    let mut judge = Judge::new(step);
    loop {
        let seen = lab.events().len();
        lab.step();
        let player = lab.rider(PLAYER).expect("a parsed step has a player");
        let state = RiderState {
            alive: player.alive,
            position: player.position,
            speed: player.speed,
            colliding: player.colliding,
            rubber: player.rubber,
        };
        let deaths: Vec<Death> = lab.events()[seen..]
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::Death { cause, .. } => Some(Death {
                    victim: &event.rider,
                    killer: cause.strip_prefix("trail:"),
                }),
                _ => None,
            })
            .collect();
        if let Some(outcome) = judge.observe(&state, &deaths) {
            return outcome;
        }
    }
}

/// The lab run of `step` with the player riding `script`: the step seed
/// (`scene::step::SEED`), no respawns, until the step's timeout.
pub fn scenario(step: &Step, script: &BTreeMap<u32, Action>) -> Scenario {
    let mut scene = step.scene.clone();
    scene.riders[step.player].script = script.clone();
    Scenario {
        until: step.timeout,
        arena: None,
        seed: scene::step::SEED,
        rules: None,
        tier: None,
        respawn_after: vec![None; scene.riders.len()],
        scene,
    }
}
