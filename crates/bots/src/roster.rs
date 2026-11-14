//! Roster files: a list of complete bot builds in TOML, written by the lab
//! (`lab ladder`, `lab tune`) and loadable by anything that seats bots.
//!
//! ```toml
//! [[build]]
//! name = "Rung 1"
//! personality = "Trapper"
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
//! The default roster of the game is `rosters/default.toml` in this crate; the
//! lab reads the same file by path.
//!
//! A roster is exactly `Vec<BotBuild>` in serde form: every knob and weight is
//! written out, so a roster keeps playing the same after the presets change.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::config::BotBuild;

#[derive(Deserialize)]
struct File {
    build: Vec<BotBuild>,
}

/// Parse a roster; names must be unique and there must be at least one build.
pub fn parse(text: &str) -> Result<Vec<BotBuild>, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    if file.build.is_empty() {
        return Err("roster has no [[build]]".into());
    }
    let mut names = HashSet::new();
    if let Some(build) = file.build.iter().find(|b| !names.insert(b.name.as_str())) {
        return Err(format!("duplicate build name {:?}", build.name));
    }
    Ok(file.build)
}

/// The roster the server seats by default, embedded at build time
/// (`rosters/default.toml`: the difficulty ladder and the artists).
pub fn builtin() -> Result<Vec<BotBuild>, String> {
    parse(include_str!("../rosters/default.toml")).map_err(|e| format!("built-in roster: {e}"))
}

/// Read and parse a roster file.
pub fn load(path: &Path) -> Result<Vec<BotBuild>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}
