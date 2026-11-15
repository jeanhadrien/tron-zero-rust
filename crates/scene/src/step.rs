//! Tutorial steps: a scene, what the player must do in it, what fails the
//! attempt, and a solution the lab checks. Times are in seconds.
//!
//! ```toml
//! [[step]]
//! prompt = "Reach the marked square."
//! timeout = 20                      # seconds from placement
//! solution = ["2.9 left"]           # the player's turns that pass (lab check)
//! must_fail = [["1.5 left"]]        # optional: near misses that must not pass
//! goal = { kind = "reach", area = [300, 200, 600, 500] }
//! # or { kind = "rubber_spent_then_alive" }, { kind = "speed_at_least", speed = 1.5 },
//! #    { kind = "kill", rider = "bot" }
//! fail = { leave = [-900, -900, 900, 900], rider_dead = ["guide"] }  # optional
//!
//! [step.scene]                      # a `Scene`, times in seconds
//! [[step.scene.rider]]
//! name = "player"                   # the rider the human plays
//! at = [-600, -600]
//! heading = "+x"
//! ```
//!
//! Death and timeout always fail. [`Judge`] decides an attempt from rider
//! states each tick; every runner (server, lab) feeds the same one.

use std::collections::BTreeMap;

use glam::Vec2;
use serde::Deserialize;
use shared::ArenaSize;
use toml::Table;

use crate::{Action, Scene, Time, parse_script};

/// The scene rider the human plays.
pub const PLAYER: &str = "player";
/// Every runner places a step's scene with this seed (`Scene::place`): the
/// same spawns at every attempt, on the server as in the lab.
pub const SEED: u64 = 0;
/// How long the player must stay alive after a wall contact ends.
pub const SURVIVE_SECS: f64 = 1.5;
const SURVIVE_TICKS: u32 = (SURVIVE_SECS * shared::TICK_HZ) as u32;

/// The cruise speed grinding one wall at contact settles at (spec §4:
/// `1 + GRIND_ACCEL_PER_SEC / SPEED_DECAY_PER_SEC * grind`). A speed goal
/// must be above it, or riding along the arena edge passes.
const WALL_TERMINAL_SPEED: f32 =
    1.0 + shared::GRIND_ACCEL_PER_SEC / shared::SPEED_DECAY_PER_SEC * shared::ARENA_WALL_GRIND;

/// The tutorial the client ships.
pub const BUILTIN: &str = include_str!("../tutorials/basics.toml");

/// An axis-aligned area of the arena.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub min: Vec2,
    pub max: Vec2,
}

impl Area {
    /// From two opposite corners `[x1, y1, x2, y2]`, in any order.
    fn from_corners([x1, y1, x2, y2]: [f32; 4]) -> Self {
        let (a, b) = (Vec2::new(x1, y1), Vec2::new(x2, y2));
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
}

#[derive(Clone, Debug)]
pub struct Step {
    pub prompt: String,
    /// Ticks the player has, counted from placement.
    pub timeout: u32,
    pub goal: Goal,
    pub fails: Fails,
    /// The player's script that passes, counted from placement.
    pub solution: BTreeMap<u32, Action>,
    /// Near misses: player scripts that must not pass (lab check).
    pub must_fail: Vec<BTreeMap<u32, Action>>,
    pub scene: Scene,
    /// Index of the [`PLAYER`] rider in `scene.riders`.
    pub player: usize,
}

impl Step {
    pub fn player(&self) -> &crate::Rider {
        &self.scene.riders[self.player]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Goal {
    /// The player's head inside `area`.
    Reach { area: Area },
    /// The player lost rubber against an obstacle, then stayed alive
    /// `SURVIVE_SECS` after the contact ended.
    RubberSpentThenAlive,
    /// The player's speed multiplier reached `speed` once.
    SpeedAtLeast { speed: f32 },
    /// `rider` died on the player's trail.
    Kill { rider: String },
}

impl Goal {
    /// The area the client marks, if the goal has one.
    pub fn area(&self) -> Option<Area> {
        match self {
            Self::Reach { area } => Some(*area),
            _ => None,
        }
    }
}

/// Fail conditions on top of death and timeout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fails {
    /// The player must stay inside this area.
    pub leave: Option<Area>,
    /// These riders must stay alive.
    pub rider_dead: Vec<String>,
}

/// Why an attempt failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    Death,
    Timeout,
    Leave,
    RiderDead(String),
}

impl Fail {
    pub fn label(&self) -> String {
        match self {
            Self::Death => "You crashed.".into(),
            Self::Timeout => "Out of time.".into(),
            Self::Leave => "You left the area.".into(),
            Self::RiderDead(name) => format!("{name} crashed."),
        }
    }
}

/// The player as a runner sees it at the end of a tick.
#[derive(Clone, Copy, Debug)]
pub struct RiderState {
    pub alive: bool,
    pub position: Vec2,
    /// Speed multiplier.
    pub speed: f32,
    /// In the rubber zone of an obstacle ahead (the sim's `IsColliding`).
    pub colliding: bool,
    pub rubber: f32,
}

/// A death this tick: `killer` owns the trail the victim died on, if any.
#[derive(Clone, Copy, Debug)]
pub struct Death<'a> {
    pub victim: &'a str,
    pub killer: Option<&'a str>,
}

/// Decides one attempt. Feed it once per simulated tick from placement.
#[derive(Clone, Debug)]
pub struct Judge {
    goal: Goal,
    fails: Fails,
    timeout: u32,
    /// Ticks observed.
    ticks: u32,
    /// Ticks since the last contact that cost rubber, once there was one.
    since_contact: Option<u32>,
    /// The player's rubber at the previous tick.
    rubber: Option<f32>,
}

impl Judge {
    pub fn new(step: &Step) -> Self {
        Self {
            goal: step.goal.clone(),
            fails: step.fails.clone(),
            timeout: step.timeout,
            ticks: 0,
            since_contact: None,
            rubber: None,
        }
    }

    pub fn ticks_left(&self) -> u32 {
        self.timeout.saturating_sub(self.ticks)
    }

    /// Judge the end of one tick from the player's state and the tick's
    /// deaths. `None` while the attempt goes on.
    pub fn observe(&mut self, player: &RiderState, deaths: &[Death]) -> Option<Result<(), Fail>> {
        self.ticks += 1;
        // The edge of the rubber zone can count as contact with nothing paid.
        let paid = player.colliding && self.rubber.is_some_and(|before| player.rubber < before);
        self.rubber = Some(player.rubber);
        if !player.alive {
            return Some(Err(Fail::Death));
        }
        if let Some(name) = self
            .fails
            .rider_dead
            .iter()
            .find(|name| deaths.iter().any(|d| d.victim == name.as_str()))
        {
            return Some(Err(Fail::RiderDead(name.clone())));
        }
        if self
            .fails
            .leave
            .is_some_and(|area| !area.contains(player.position))
        {
            return Some(Err(Fail::Leave));
        }
        let passed = match &self.goal {
            Goal::Reach { area } => area.contains(player.position),
            Goal::RubberSpentThenAlive => {
                if paid {
                    self.since_contact = Some(0);
                } else if let Some(ticks) = &mut self.since_contact {
                    *ticks += 1;
                }
                self.since_contact.is_some_and(|t| t >= SURVIVE_TICKS)
            }
            Goal::SpeedAtLeast { speed } => player.speed >= *speed,
            Goal::Kill { rider } => deaths
                .iter()
                .any(|d| d.victim == rider.as_str() && d.killer == Some(PLAYER)),
        };
        if passed {
            return Some(Ok(()));
        }
        (self.ticks >= self.timeout).then_some(Err(Fail::Timeout))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    step: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    prompt: String,
    timeout: f64,
    goal: RawGoal,
    #[serde(default)]
    fail: RawFails,
    solution: Vec<String>,
    #[serde(default)]
    must_fail: Vec<Vec<String>>,
    scene: Table,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawGoal {
    Reach { area: [f32; 4] },
    // A struct variant: a unit variant would ignore unknown keys.
    RubberSpentThenAlive {},
    SpeedAtLeast { speed: f32 },
    Kill { rider: String },
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFails {
    #[serde(default)]
    leave: Option<[f32; 4]>,
    #[serde(default)]
    rider_dead: Vec<String>,
}

/// Parse and check a step file for the default arena: at least one step,
/// hosted scene limits, one `player` rider with neither bot nor script,
/// areas inside the arena, goals that cannot pass at placement, and names
/// that exist.
pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let file: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
    if file.step.is_empty() {
        return Err("no [[step]]".into());
    }
    file.step
        .into_iter()
        .enumerate()
        .map(|(i, raw)| raw.resolve().map_err(|e| format!("step {}: {e}", i + 1)))
        .collect()
}

impl RawStep {
    fn resolve(self) -> Result<Step, String> {
        let size = ArenaSize::default();
        let scene = Scene::from_table(self.scene, Time::Seconds)?;
        scene.check_limits(size)?;
        let index = scene
            .riders
            .iter()
            .position(|rider| rider.name == PLAYER)
            .ok_or(format!("no rider named {PLAYER:?}"))?;
        let player = &scene.riders[index];
        if player.bot.is_some() || !player.script.is_empty() {
            return Err(format!(
                "{PLAYER:?} is played by the human: no bot, no script"
            ));
        }
        let timeout = Time::Seconds.ticks(self.timeout)?;
        if timeout == 0 {
            return Err("timeout must be above 0".into());
        }
        let half = Vec2::new(size.width, size.height) * 0.5;
        let area = |corners: [f32; 4]| {
            let area = Area::from_corners(corners);
            let inside = area.min.cmpge(-half).all() && area.max.cmple(half).all();
            if !(area.min.cmplt(area.max).all() && inside) {
                return Err(format!("area {corners:?} is empty or leaves the arena"));
            }
            Ok(area)
        };
        let other = |name: &str| {
            if name == PLAYER || !scene.riders.iter().any(|rider| rider.name == name) {
                return Err(format!("{name:?} is not a rider other than the player"));
            }
            Ok(())
        };
        let goal = match self.goal {
            RawGoal::Reach { area: corners } => Goal::Reach {
                area: area(corners)?,
            },
            RawGoal::RubberSpentThenAlive {} => Goal::RubberSpentThenAlive,
            RawGoal::SpeedAtLeast { speed } => {
                let floor = player.speed.max(WALL_TERMINAL_SPEED);
                if !(speed > floor && speed <= shared::MAX_SPEED_MULT) {
                    return Err(format!(
                        "speed {speed} must be above {floor} (start speed, one wall's grind)                          and at most {}",
                        shared::MAX_SPEED_MULT
                    ));
                }
                Goal::SpeedAtLeast { speed }
            }
            RawGoal::Kill { rider } => {
                other(&rider)?;
                if self.fail.rider_dead.contains(&rider) {
                    return Err(format!("{rider:?} is both the kill and a rider_dead fail"));
                }
                Goal::Kill { rider }
            }
        };
        for name in &self.fail.rider_dead {
            other(name)?;
        }
        let fails = Fails {
            leave: self.fail.leave.map(area).transpose()?,
            rider_dead: self.fail.rider_dead,
        };
        Ok(Step {
            prompt: self.prompt,
            timeout,
            goal,
            fails,
            solution: parse_script(PLAYER, &self.solution, Time::Seconds)?,
            must_fail: self
                .must_fail
                .iter()
                .map(|script| parse_script(PLAYER, script, Time::Seconds))
                .collect::<Result<_, _>>()?,
            scene,
            player: index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER_SCENE: &str =
        "[step.scene]\n[[step.scene.rider]]\nname = \"player\"\nat = [0, 0]\nheading = \"+x\"\n";

    fn step(goal: &str, extra: &str) -> Result<Vec<Step>, String> {
        parse(&format!(
            "[[step]]\nprompt = \"p\"\ntimeout = 2\nsolution = []\ngoal = {goal}\n{extra}{PLAYER_SCENE}"
        ))
    }

    fn rider(position: Vec2) -> RiderState {
        RiderState {
            alive: true,
            position,
            speed: 1.0,
            colliding: false,
            rubber: shared::BASE_RUBBER,
        }
    }

    fn judge(goal: Goal, fails: Fails) -> Judge {
        Judge {
            goal,
            fails,
            timeout: 10,
            ticks: 0,
            since_contact: None,
            rubber: None,
        }
    }

    #[test]
    fn the_shipped_tutorial_loads() {
        assert!(!parse(BUILTIN).unwrap().is_empty());
    }

    #[test]
    fn files_are_checked_where_a_runner_would_fail_late() {
        let reach = "{ kind = \"reach\", area = [100, 100, 200, 200] }";
        let ok = step(reach, "").unwrap();
        assert_eq!(ok[0].timeout, 240, "seconds become ticks");
        assert!(step("{ kind = \"reach\", area = [100, 100, 100, 200] }", "").is_err());
        assert!(step("{ kind = \"reach\", area = [900, 900, 1100, 1100] }", "").is_err());
        assert!(step("{ kind = \"speed_at_least\", speed = 1.0 }", "").is_err());
        assert!(step("{ kind = \"speed_at_least\", speed = 9.0 }", "").is_err());
        assert!(step("{ kind = \"speed_at_least\", speed = 1.4 }", "").is_err());
        assert!(step("{ kind = \"speed_at_least\", speed = 1.5 }", "").is_ok());
        assert!(step("{ kind = \"kill\", rider = \"player\" }", "").is_err());
        assert!(step(reach, "fail = { rider_dead = [\"ghost\"] }\n").is_err());
        assert!(step("{ kind = \"fly\" }", "").is_err());
        assert!(step("{ kind = \"rubber_spent_then_alive\", x = 1 }", "").is_err());
        let scripted = PLAYER_SCENE.to_owned() + "script = [\"1 left\"]\n";
        let text = format!(
            "[[step]]\nprompt = \"p\"\ntimeout = 2\nsolution = []\ngoal = {reach}\n{scripted}"
        );
        assert!(parse(&text).unwrap_err().contains("no bot, no script"));
    }

    #[test]
    fn death_fails_first_and_reaching_passes() {
        let area = Area::from_corners([10.0, -10.0, 20.0, 10.0]);
        let mut j = judge(Goal::Reach { area }, Fails::default());
        assert_eq!(j.observe(&rider(Vec2::ZERO), &[]), None);
        assert_eq!(j.observe(&rider(Vec2::new(15.0, 0.0)), &[]), Some(Ok(())));
        let dead = RiderState {
            alive: false,
            ..rider(Vec2::new(15.0, 0.0))
        };
        let mut j = judge(Goal::Reach { area }, Fails::default());
        assert_eq!(j.observe(&dead, &[]), Some(Err(Fail::Death)));
    }

    #[test]
    fn idling_times_out_and_leaving_fails() {
        let far = Area::from_corners([500.0, 500.0, 600.0, 600.0]);
        let mut j = judge(Goal::Reach { area: far }, Fails::default());
        let outcomes: Vec<_> = (0..10)
            .map(|_| j.observe(&rider(Vec2::ZERO), &[]))
            .collect();
        assert_eq!(outcomes[8], None);
        assert_eq!(outcomes[9], Some(Err(Fail::Timeout)));
        let fails = Fails {
            leave: Some(Area::from_corners([-5.0, -5.0, 5.0, 5.0])),
            ..Fails::default()
        };
        let mut j = judge(Goal::Reach { area: far }, fails);
        assert_eq!(
            j.observe(&rider(Vec2::new(6.0, 0.0)), &[]),
            Some(Err(Fail::Leave))
        );
    }

    #[test]
    fn rubber_passes_only_after_surviving_the_contact() {
        let mut j = judge(Goal::RubberSpentThenAlive, Fails::default());
        j.timeout = 10 * SURVIVE_TICKS;
        let free = rider(Vec2::ZERO);
        let grazing = RiderState {
            colliding: true,
            ..free
        };
        let pressing = RiderState {
            rubber: free.rubber - 1.0,
            ..grazing
        };
        assert_eq!(j.observe(&free, &[]), None, "no contact yet");
        assert_eq!(j.observe(&grazing, &[]), None);
        for _ in 0..SURVIVE_TICKS {
            assert_eq!(j.observe(&free, &[]), None, "a contact that cost nothing");
        }
        assert_eq!(j.observe(&pressing, &[]), None);
        for _ in 1..SURVIVE_TICKS {
            assert_eq!(j.observe(&free, &[]), None);
        }
        assert_eq!(j.observe(&free, &[]), Some(Ok(())));
    }

    #[test]
    fn speed_is_reached_once() {
        let mut j = judge(Goal::SpeedAtLeast { speed: 1.5 }, Fails::default());
        let fast = RiderState {
            speed: 1.5,
            ..rider(Vec2::ZERO)
        };
        assert_eq!(j.observe(&rider(Vec2::ZERO), &[]), None);
        assert_eq!(j.observe(&fast, &[]), Some(Ok(())));
    }

    #[test]
    fn a_kill_needs_the_players_trail_and_a_guarded_rider_must_live() {
        let goal = Goal::Kill {
            rider: "bot".into(),
        };
        let riders = rider(Vec2::ZERO);
        let wall = Death {
            victim: "bot",
            killer: None,
        };
        let mine = Death {
            victim: "bot",
            killer: Some(PLAYER),
        };
        let mut j = judge(goal.clone(), Fails::default());
        assert_eq!(j.observe(&riders, &[wall]), None);
        assert_eq!(j.observe(&riders, &[mine]), Some(Ok(())));
        let fails = Fails {
            rider_dead: vec!["guide".into()],
            ..Fails::default()
        };
        let guide = Death {
            victim: "guide",
            killer: None,
        };
        let mut j = judge(goal, fails);
        assert_eq!(
            j.observe(&riders, &[guide, mine]),
            Some(Err(Fail::RiderDead("guide".into())))
        );
    }
}
