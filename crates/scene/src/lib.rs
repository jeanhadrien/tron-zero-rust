//! Scenes: what is set up on the arena before riders ride. Extra walls, and
//! per rider a start (a fixed pose or a safe spawn), a speed, a rubber level, a
//! trail already laid, and a script of turns or a bot build.
//!
//! ```toml
//! walls = [[-200, 100, 200, 100]]  # optional extra segments x1 y1 x2 y2
//!
//! [[rider]]
//! name = "a"
//! at = [-1000, 0]                  # with `heading`: a fixed start;
//! heading = "+x"                   # without both, a safe spawn. +x | -x | +y | -y (y up)
//! speed = 1.0                      # optional starting cruise speed
//! rubber = 120                     # optional starting rubber
//! trail = [[-1000, -300]]          # optional corners laid already, oldest first
//! script = ["40 left", "100 right"] # "<tick> left|right|respawn", tick 0 = placement
//!
//! [[rider]]
//! name = "trapper"
//! bot = "trapper"                  # a bot build (`bots::spec::BuildSpec`) instead of a script
//! difficulty = 2500
//! [rider.knobs]
//! reaction_ticks = 4
//! ```
//!
//! [`Scene::from_table`] checks what any runner needs (names, starts, scripts,
//! builds). [`Scene::check_limits`] adds what a hosted scene must also hold:
//! caps, in-bounds poses and walls, trails the game could have laid, no
//! respawn.

use std::collections::{BTreeMap, HashSet};

use bots::config::{BotBuild, Personality};
use bots::spec::{BuildSpec, DEFAULT_DIFFICULTY};
use glam::Vec2;
use serde::Deserialize;
use shared::ArenaSize;
use shared::spawn::{SafeSpawn, Start};
use toml::{Table, Value};

/// Hosted scene caps (`Scene::check_limits`).
pub const MAX_WALLS: usize = 64;
pub const MAX_RIDERS: usize = 8;
pub const MAX_TRAIL_CORNERS: usize = 64;
pub const MAX_SCRIPT_ENTRIES: usize = 256;

#[derive(Clone, Debug)]
pub struct Scene {
    /// Extra segments `[x1, y1, x2, y2]`, on top of the arena boundary.
    pub walls: Vec<[f32; 4]>,
    pub riders: Vec<Rider>,
}

#[derive(Clone, Debug)]
pub struct Rider {
    pub name: String,
    /// A fixed start; `None` asks for a safe spawn.
    pub at: Option<SafeSpawn>,
    pub speed: f32,
    pub rubber: f32,
    /// Corners laid already, oldest first; the last leg runs to `at`.
    pub trail: Vec<Vec2>,
    /// One action per tick, counted from the tick the rider is placed.
    pub script: BTreeMap<u32, Action>,
    pub bot: Option<BotBuild>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Left,
    Right,
    Respawn,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Heading {
    #[serde(rename = "+x")]
    PosX,
    #[serde(rename = "-x")]
    NegX,
    #[serde(rename = "+y")]
    PosY,
    #[serde(rename = "-y")]
    NegY,
}

impl Heading {
    pub fn vector(self) -> Vec2 {
        match self {
            Self::PosX => Vec2::X,
            Self::NegX => Vec2::NEG_X,
            Self::PosY => Vec2::Y,
            Self::NegY => Vec2::NEG_Y,
        }
    }

    /// Scene spelling of a cardinal sim heading (`"?"` if not cardinal).
    pub fn label(direction: Vec2) -> &'static str {
        [Self::PosX, Self::NegX, Self::PosY, Self::NegY]
            .into_iter()
            .find(|heading| heading.vector() == direction)
            .map_or("?", |heading| match heading {
                Self::PosX => "+x",
                Self::NegX => "-x",
                Self::PosY => "+y",
                Self::NegY => "-y",
            })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScene {
    #[serde(default)]
    walls: Vec<[f32; 4]>,
    #[serde(rename = "rider")]
    riders: Vec<RawRider>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRider {
    name: String,
    #[serde(default)]
    at: Option<[f32; 2]>,
    #[serde(default)]
    heading: Option<Heading>,
    #[serde(default = "default_speed")]
    speed: f32,
    #[serde(default = "default_rubber")]
    rubber: f32,
    #[serde(default)]
    trail: Vec<[f32; 2]>,
    #[serde(default)]
    script: Vec<String>,
    #[serde(default)]
    bot: Option<Personality>,
    #[serde(default = "default_difficulty")]
    difficulty: u32,
    #[serde(default)]
    knobs: Table,
    #[serde(default)]
    weights: Table,
}

fn default_speed() -> f32 {
    1.0
}

fn default_rubber() -> f32 {
    shared::BASE_RUBBER
}

fn default_difficulty() -> u32 {
    DEFAULT_DIFFICULTY
}

impl Scene {
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::from_table(toml::from_str(text).map_err(|e| e.to_string())?)
    }

    /// A scene from its TOML table: every key must be a scene key.
    pub fn from_table(table: Table) -> Result<Self, String> {
        let raw: RawScene = Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        let mut names = HashSet::new();
        let riders = raw
            .riders
            .into_iter()
            .map(|rider| {
                if !names.insert(rider.name.clone()) {
                    return Err(format!("duplicate rider name {:?}", rider.name));
                }
                rider.resolve()
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            walls: raw.walls,
            riders,
        })
    }

    /// What a hosted scene must hold on top of `from_table`: the caps, walls
    /// and starts inside an arena of `size`, trails the game could have laid
    /// (cardinal legs, within `TRAIL_MAX_LENGTH`), sane speed and rubber, and
    /// scripts that only turn.
    pub fn check_limits(&self, size: ArenaSize) -> Result<(), String> {
        let half = Vec2::new(size.width, size.height) * 0.5;
        let inside = |p: Vec2| p.is_finite() && p.abs().cmplt(half).all();
        if self.walls.len() > MAX_WALLS {
            return Err(format!("more than {MAX_WALLS} walls"));
        }
        if self.riders.len() > MAX_RIDERS {
            return Err(format!("more than {MAX_RIDERS} riders"));
        }
        for &[x1, y1, x2, y2] in &self.walls {
            let (a, b) = (Vec2::new(x1, y1), Vec2::new(x2, y2));
            let on = |p: Vec2| p.is_finite() && p.abs().cmple(half).all();
            if !on(a) || !on(b) {
                return Err(format!("wall {:?} leaves the arena", [x1, y1, x2, y2]));
            }
        }
        for rider in &self.riders {
            let fail = |what: &str| Err(format!("{}: {what}", rider.name));
            if !(rider.speed > 0.0 && rider.speed <= shared::MAX_SPEED_MULT) {
                return fail("speed out of range");
            }
            if !(rider.rubber > 0.0 && rider.rubber <= shared::BASE_RUBBER) {
                return fail("rubber out of range");
            }
            if rider.script.len() > MAX_SCRIPT_ENTRIES {
                return fail("script too long");
            }
            if rider.script.values().any(|a| *a == Action::Respawn) {
                return fail("a hosted scene never respawns");
            }
            if rider.trail.len() > MAX_TRAIL_CORNERS {
                return fail("too many trail corners");
            }
            let Some(at) = rider.at else { continue };
            if !inside(at.position) {
                return fail("starts outside the arena");
            }
            let mut points = rider.trail.clone();
            points.push(at.position);
            if !points.iter().all(|&p| inside(p)) {
                return fail("trail leaves the arena");
            }
            if points
                .windows(2)
                .any(|leg| leg[0].x != leg[1].x && leg[0].y != leg[1].y)
            {
                return fail("a trail leg is not along x or y");
            }
            if points.len() > 1 && shared::polyline::length(&points) > shared::TRAIL_MAX_LENGTH {
                return fail("trail longer than TRAIL_MAX_LENGTH");
            }
        }
        Ok(())
    }
}

impl Rider {
    /// This rider's start once placed at `spawn` (its own pose, or the safe
    /// spawn found for it).
    pub fn start(&self, spawn: SafeSpawn) -> Start {
        Start {
            spawn,
            speed: self.speed,
            rubber: self.rubber,
            trail: self.trail.clone(),
        }
    }
}

impl RawRider {
    fn resolve(self) -> Result<Rider, String> {
        let name = self.name;
        let at = match (self.at, self.heading) {
            (Some(at), Some(heading)) => Some(SafeSpawn {
                position: Vec2::from(at),
                direction: heading.vector(),
            }),
            (None, None) => None,
            _ => return Err(format!("{name}: give both `at` and `heading`, or neither")),
        };
        if at.is_none() && !self.trail.is_empty() {
            return Err(format!("{name}: a trail needs a fixed `at`"));
        }
        let script = parse_script(&name, &self.script)?;
        let bot = match self.bot {
            Some(bot) => {
                if script.values().any(|a| *a != Action::Respawn) {
                    return Err(format!("{name}: a bot's script may only respawn"));
                }
                let spec = BuildSpec {
                    name: name.clone(),
                    bot,
                    difficulty: self.difficulty,
                    knobs: self.knobs,
                    weights: self.weights,
                };
                Some(spec.build()?)
            }
            None => None,
        };
        Ok(Rider {
            name,
            at,
            speed: self.speed,
            rubber: self.rubber,
            trail: self.trail.into_iter().map(Vec2::from).collect(),
            script,
            bot,
        })
    }
}

/// Parse `"<tick> <action>"` entries into one action per tick. The sim
/// applies at most one turn per tick, so two entries on a tick is an error.
fn parse_script(name: &str, entries: &[String]) -> Result<BTreeMap<u32, Action>, String> {
    let mut script = BTreeMap::new();
    for entry in entries {
        let mut parts = entry.split_whitespace();
        let (Some(tick), Some(action), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!(
                "{name}: expected \"<tick> <action>\", got {entry:?}"
            ));
        };
        let tick: u32 = tick
            .parse()
            .map_err(|_| format!("{name}: bad tick in {entry:?}"))?;
        let action = match action.to_ascii_lowercase().as_str() {
            "l" | "left" => Action::Left,
            "r" | "right" => Action::Right,
            "respawn" => Action::Respawn,
            other => return Err(format!("{name}: unknown action {other:?}")),
        };
        if script.insert(tick, action).is_some() {
            return Err(format!("{name}: two actions on tick {tick}"));
        }
    }
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosted(text: &str) -> Result<(), String> {
        Scene::parse(text)?.check_limits(ArenaSize::default())
    }

    #[test]
    fn a_rider_needs_both_pose_fields_or_none() {
        let half = "[[rider]]\nname = \"a\"\nat = [0, 0]";
        assert!(Scene::parse(half).unwrap_err().contains("both"));
        let safe = Scene::parse("[[rider]]\nname = \"a\"").unwrap();
        assert!(safe.riders[0].at.is_none());
    }

    #[test]
    fn unknown_keys_and_bad_scripts_fail() {
        assert!(Scene::parse("[[rider]]\nname = \"a\"\nsped = 2.0").is_err());
        assert!(
            Scene::parse("[[rider]]\nname = \"a\"\nscript = [\"5 left\", \"5 right\"]").is_err()
        );
        let bot = "[[rider]]\nname = \"a\"\nbot = \"duelist\"\nscript = [\"5 left\"]";
        assert!(Scene::parse(bot).unwrap_err().contains("only respawn"));
    }

    #[test]
    fn hosted_scenes_keep_trails_the_game_could_lay() {
        let rider = |trail: &str| {
            format!("[[rider]]\nname = \"a\"\nat = [0, 0]\nheading = \"+x\"\ntrail = {trail}")
        };
        assert!(hosted(&rider("[[-300, 200], [-300, 0]]")).is_ok());
        assert!(
            hosted(&rider("[[-300, 200]]"))
                .unwrap_err()
                .contains("along x or y")
        );
        let long = rider("[[-990, 990], [990, 990], [990, 0]]");
        assert!(hosted(&long).unwrap_err().contains("longer"));
        assert!(hosted(&rider("[[-1200, 0]]")).is_err(), "outside");
    }

    #[test]
    fn hosted_scenes_never_respawn_and_stay_capped() {
        let respawn = "[[rider]]\nname = \"a\"\nscript = [\"5 respawn\"]";
        assert!(Scene::parse(respawn).is_ok());
        assert!(hosted(respawn).is_err());
        let many: String = (0..=MAX_RIDERS)
            .map(|i| format!("[[rider]]\nname = \"r{i}\"\n"))
            .collect();
        assert!(hosted(&many).is_err());
    }
}
