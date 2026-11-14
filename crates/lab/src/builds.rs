//! Bot builds in file vocabulary, and parameters addressed by name.
//!
//! One shape describes a build in suites, tune and ladder files and their
//! output:
//!
//! ```toml
//! [[build]]                  # or [[pool]] / [base], by file
//! name = "Tuned"
//! bot = "trapper"            # personality, lowercase; `personality` also works
//! difficulty = 2500          # optional: the authoring preset the knobs start from
//! [build.knobs]              # optional: any knob of `bots::config::BotKnobs`
//! noise = 0.0
//! [build.weights]            # optional: any weight of `bots::objective::Weights`
//! pressure = 3.0
//! ```
//!
//! Knobs and weights are matched by name against the serde form of the bots
//! crate's types, so a knob or weight added or renamed there needs no change
//! here. Roster files (`bots::roster`) are the same shape with every knob and
//! weight written out.

use std::collections::BTreeMap;
use std::path::Path;

use bots::config::{BotBuild, BotKnobs, Difficulty, Personality};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use toml::{Table, Value};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    pub name: String,
    #[serde(alias = "personality")]
    pub bot: Personality,
    #[serde(default = "default_difficulty")]
    pub difficulty: u32,
    #[serde(default)]
    pub knobs: Table,
    #[serde(default)]
    pub weights: Table,
}

/// The `roster` value that names the server's embedded roster.
const BUILTIN: &str = "builtin";

fn default_difficulty() -> u32 {
    1500
}

/// `base` with the entries of `over` replacing same-named fields. Unknown
/// names and wrongly typed values are errors.
fn merged<T: Serialize + DeserializeOwned>(
    base: &T,
    over: &Table,
    what: &str,
) -> Result<T, String> {
    let mut table = Table::try_from(base).map_err(|e| e.to_string())?;
    for (key, value) in over {
        if !table.contains_key(key) {
            let known: Vec<&String> = table.keys().collect();
            return Err(format!("unknown {what} {key:?}, known: {known:?}"));
        }
        table.insert(key.clone(), value.clone());
    }
    Value::Table(table)
        .try_into()
        .map_err(|e| format!("{what}: {e}"))
}

impl BuildSpec {
    pub fn build(&self) -> Result<BotBuild, String> {
        let mut knobs: BotKnobs = merged(&Difficulty(self.difficulty).knobs(), &self.knobs, "knob")
            .map_err(|e| format!("{}: {e}", self.name))?;
        knobs.clamp();
        let mut build = BotBuild::new(&self.name, self.bot, knobs);
        build.weights = merged(&build.weights, &self.weights, "weight")
            .map_err(|e| format!("{}: {e}", self.name))?;
        Ok(build)
    }
}

/// Where a file's builds come from: a roster file (relative paths resolve
/// against `base`; `"builtin"` is the roster the server seats, embedded in the
/// bots crate) and inline specs, in this order.
pub fn resolve(
    roster: Option<&Path>,
    specs: &[BuildSpec],
    base: &Path,
) -> Result<Vec<BotBuild>, String> {
    let mut builds = match roster {
        Some(path) if path == Path::new(BUILTIN) => bots::roster::builtin()?,
        Some(path) => bots::roster::load(&base.join(path))?,
        None => Vec::new(),
    };
    for spec in specs {
        builds.push(spec.build()?);
    }
    let mut names = std::collections::HashSet::new();
    if let Some(build) = builds.iter().find(|b| !names.insert(b.name.as_str())) {
        return Err(format!("duplicate build name {:?}", build.name));
    }
    Ok(builds)
}

/// One tunable number of a build.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub value: f64,
    /// Whole-number knob (ticks, variety): set values are rounded.
    pub integer: bool,
}

fn table_params(table: &Table, out: &mut BTreeMap<String, Param>) {
    for (name, value) in table {
        let param = match value {
            Value::Integer(v) => Param {
                value: *v as f64,
                integer: true,
            },
            Value::Float(v) => Param {
                value: *v,
                integer: false,
            },
            _ => continue,
        };
        out.insert(name.clone(), param);
    }
}

fn tables(build: &BotBuild) -> Result<(Table, Table), String> {
    Ok((
        Table::try_from(&build.knobs).map_err(|e| e.to_string())?,
        Table::try_from(build.weights).map_err(|e| e.to_string())?,
    ))
}

/// Every numeric knob and weight of a build by name. A name used by both is
/// an error: parameters are addressed by bare name.
pub fn params(build: &BotBuild) -> Result<BTreeMap<String, Param>, String> {
    let (knobs, weights) = tables(build)?;
    let mut out = BTreeMap::new();
    table_params(&knobs, &mut out);
    let before = out.len();
    table_params(&weights, &mut out);
    if out.len() != before + weights.len() {
        return Err("a knob and a weight share a name".into());
    }
    Ok(out)
}

/// `build` with the named knobs and weights set (knobs re-clamped).
pub fn with_params(build: &BotBuild, set: &[(String, f64)]) -> Result<BotBuild, String> {
    let known = params(build)?;
    let (own_knobs, _) = tables(build)?;
    let (mut knobs, mut weights) = (Table::new(), Table::new());
    for (name, value) in set {
        let param = known
            .get(name)
            .ok_or_else(|| format!("unknown parameter {name:?}"))?;
        let value = if param.integer {
            Value::Integer(value.round() as i64)
        } else {
            Value::Float(*value)
        };
        if own_knobs.contains_key(name) {
            knobs.insert(name.clone(), value);
        } else {
            weights.insert(name.clone(), value);
        }
    }
    let mut out = build.clone();
    out.knobs = merged(&build.knobs, &knobs, "knob")?;
    out.knobs.clamp();
    out.weights = merged(&build.weights, &weights, "weight")?;
    Ok(out)
}

/// Names of the budget knobs (`bots::config::BotKnobs`).
pub fn knob_names() -> Result<Vec<String>, String> {
    let knobs = Table::try_from(Difficulty(1500).knobs()).map_err(|e| e.to_string())?;
    Ok(knobs.keys().cloned().collect())
}

/// Knob values of the authoring preset at `level`.
pub fn preset_knobs(level: u32) -> Result<BTreeMap<String, Param>, String> {
    let mut out = BTreeMap::new();
    let table = Table::try_from(Difficulty(level).knobs()).map_err(|e| e.to_string())?;
    table_params(&table, &mut out);
    Ok(out)
}

/// Shortest decimal text of a number as stored (`f32` for weights and
/// knobs, so `0.15` prints as `0.15`).
fn number(value: &Value) -> String {
    match value {
        Value::Integer(v) => v.to_string(),
        Value::Float(v) => format!("{:?}", *v as f32),
        other => other.to_string(),
    }
}

/// A build as a `[[build]]` entry with every knob and weight written out:
/// valid in rosters and suites (rename the array to `pool` for tune and
/// ladder files).
pub fn roster_entry(build: &BotBuild) -> Result<String, String> {
    let (knobs, weights) = tables(build)?;
    let mut out = format!(
        "[[build]]\nname = {}\npersonality = {}\n",
        Value::String(build.name.clone()),
        Value::try_from(build.personality).map_err(|e| e.to_string())?
    );
    for (title, table) in [("knobs", knobs), ("weights", weights)] {
        out += &format!("[build.{title}]\n");
        for (name, value) in table {
            out += &format!("{name} = {}\n", number(&value));
        }
    }
    Ok(out)
}

/// A whole roster file.
pub fn roster_file(builds: &[BotBuild], header: &str) -> Result<String, String> {
    let mut out = String::new();
    for line in header.lines() {
        out += &format!("# {line}\n");
    }
    for build in builds {
        out += &format!("\n{}", roster_entry(build)?);
    }
    Ok(out)
}
