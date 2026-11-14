//! Roster files: the planner tuning and a list of complete bot builds in
//! TOML, written by tuning tools and loadable by anything that seats bots.
//!
//! ```toml
//! [tuning.room]        # every table and key of `tuning::Tuning`, required
//! need_cells = 300.0
//! # ...
//!
//! [[build]]
//! name = "Rung 1"
//! personality = "trapper"
//! [build.knobs]
//! reaction_ticks = 7
//! horizon_ticks = 40
//! variety = 2
//! noise = 0.28
//! [build.weights]
//! space = 1.0
//! # ... every weight of `objective::Weights`
//! ```
//!
//! The default roster of the game is `rosters/default.toml` in this crate; any
//! runner can read the same file by path.
//!
//! Every knob, weight and tuning value is written out, so a roster keeps
//! playing the same after the presets change. Every build of a file shares
//! its tuning.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::config::{BotBuild, BotKnobs, Personality};
use crate::objective::Weights;
use crate::tuning::Tuning;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    tuning: Tuning,
    build: Vec<Entry>,
}

/// A build as written in a file: everything but the tuning.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    personality: Personality,
    knobs: BotKnobs,
    weights: Weights,
}

/// Parse a roster; names must be unique and there must be at least one build.
pub fn parse(text: &str) -> Result<Vec<BotBuild>, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    if file.build.is_empty() {
        return Err("roster has no [[build]]".into());
    }
    let mut names = HashSet::new();
    if let Some(entry) = file.build.iter().find(|b| !names.insert(b.name.as_str())) {
        return Err(format!("duplicate build name {:?}", entry.name));
    }
    Ok(file
        .build
        .into_iter()
        .map(|e| BotBuild {
            name: e.name,
            personality: e.personality,
            knobs: e.knobs,
            weights: e.weights,
            tuning: file.tuning,
        })
        .collect())
}

/// The roster the game seats by default, embedded at build time
/// (`rosters/default.toml`: the difficulty ladder and the artists).
pub fn builtin() -> Result<Vec<BotBuild>, String> {
    parse(include_str!("../rosters/default.toml")).map_err(|e| format!("built-in roster: {e}"))
}

/// The tuning of the built-in roster: what a build authored outside a roster
/// file (a scenario, a suite) runs with.
pub fn builtin_tuning() -> Result<Tuning, String> {
    builtin()?
        .first()
        .map(|b| b.tuning)
        .ok_or_else(|| "built-in roster is empty".into())
}

/// Read and parse a roster file.
pub fn load(path: &Path) -> Result<Vec<BotBuild>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}
