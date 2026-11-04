//! Tron Zero lab: deterministic, headless scenario runs of the shared sim.
//!
//! A scenario (TOML) places riders and scripts their turns per tick, or hands
//! them to the server's bot brain; the lab
//! steps the real `simulate_players` system like the server does and emits a
//! JSONL event trace and PNG snapshots. Built for agents and tests that can't
//! play at 120 Hz: edit the scenario, re-run from tick 0, read the outcome.

pub mod render;
pub mod scenario;
pub mod sim;

pub use render::{PALETTE, View, render};
pub use scenario::Scenario;
pub use shared::Elimination;
pub use sim::{Event, EventKind, Lab, RiderView, Score};

/// One JSON object per line.
pub fn eliminations_jsonl(eliminations: &[Elimination<String>]) -> String {
    eliminations
        .iter()
        .map(|e| {
            serde_json::to_string(e).expect("eliminations serialize")
                + "
"
        })
        .collect()
}

/// One JSON object per line.
pub fn to_jsonl(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| serde_json::to_string(e).expect("events serialize") + "\n")
        .collect()
}

/// Run a scenario to completion and return its trace as JSONL.
pub fn trace(scenario: &Scenario, sample: u32) -> String {
    let mut lab = Lab::new(scenario);
    lab.run(scenario.until, sample);
    to_jsonl(lab.events())
}
