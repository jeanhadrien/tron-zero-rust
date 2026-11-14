//! The planner's global constants: one `[tuning]` table per roster file,
//! shared by every build of the file and part of each build's rating id.
//!
//! Every key is required: the file is the source of truth, there are no
//! defaults in code. Values are read as written (no range checks).

use serde::{Deserialize, Serialize};

use crate::schedule::Tier;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tuning {
    pub room: Room,
    pub search: Search,
    pub engage: Engage,
    pub target: Target,
    pub bold: Bold,
    pub cut: Cut,
    pub lanes: Lanes,
    pub heat: Heat,
    pub artist: Artist,
    pub tiers: Tiers,
}

/// Room and safety.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    /// Cells of room a bot wants around it; smaller regions are pockets. An
    /// opponent is sealed under a quarter of it (in coarse cells).
    pub need_cells: f32,
    /// Free ticks ahead of the end pose that count as fully clear.
    pub clear_ticks: f32,
    /// Slowest cruise assumed when tracing a line (a rider that just turned).
    pub min_cruise: f32,
    /// Rubber kept in reserve when the burn is judged against what is left.
    pub rubber_slack: f32,
    /// Riders further than `rollout_reach` units per tick of the window, plus
    /// `rollout_margin`, are left out of a physics check.
    pub rollout_reach: f32,
    pub rollout_margin: f32,
}

/// How many candidates get the costly checks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    /// Candidates the physics may check, best first, before giving up.
    pub verify_top: usize,
    /// Candidates (best first, plus riding straight) that get the territory race.
    pub race_top: usize,
    /// Ticks ahead the race looks.
    pub race_ticks: f32,
    /// Best lines whose effect on the target's replies is checked, and how
    /// close the target must be for it.
    pub reply_top: usize,
    pub reply_range: f32,
}

/// Engagement: heading towards the nearest opponent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engage {
    /// Opponents closer than this need no engagement.
    pub range: f32,
    /// Further than `range` by this much, engagement is at its worst.
    pub span: f32,
    /// Headings are told apart in steps of `1 / steps` of cosine.
    pub steps: f32,
    /// Ticks of a bot's life for engagement to count one more time over.
    pub ramp: f32,
    /// Engagement never counts more than this many times its weight.
    pub max: f32,
    /// How much an opponent's speed against the bot's scales engagement.
    pub pace_min: f32,
    pub pace_max: f32,
    /// Territory only matters with an opponent this close (units); it fades
    /// out over `contact_span` further.
    pub contact_near: f32,
    pub contact_span: f32,
}

/// The choice of whom to pressure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// A target counts as this many logs of cells closer per 1000 units away.
    pub distance: f32,
    /// Logs of cells a target with all its rubber burned is worth.
    pub rubber: f32,
    /// A new target must beat the current one by this much (logs of cells).
    pub stick: f32,
    /// Pressure at which a line counts as an attack (reported mode only).
    pub attack_pressure: f32,
}

/// Lines below the calm risk window, taken only for a kill chance.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bold {
    /// Pressure such a line needs to stay eligible.
    pub pressure: f32,
    /// Heat at which it needs no kill chance at all.
    pub fade: f32,
}

/// Cutting across the target's line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    /// Ticks to spare at least; full value up to `best_margin`, fading out
    /// over `fade` more.
    pub min_margin: f32,
    pub best_margin: f32,
    pub fade: f32,
    /// How far ahead (ticks) the bot's line is followed when looking for a cut.
    pub ticks: f32,
}

/// Grinding and the lanes beside obstacles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lanes {
    /// Speed (x base) beyond which more is not worth a detour.
    pub grind_cap: f32,
    /// How close beside an obstacle a grind line is aimed (units).
    pub grind_gap: f32,
    /// Farthest side obstacle (units) a lane towards it is worth planning.
    pub lane_reach: f32,
    /// Margins (units) a late turn ahead of an obstacle is aimed at.
    pub late_gaps: [f32; 2],
}

/// Round heat: bots grow bolder as a round of last rider standing ages.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heat {
    /// Heat is full `span` ticks after the round began, and rises to `max`.
    pub span: f32,
    pub max: f32,
    /// What a full heat adds to the fight weights and the risk.
    pub pressure: f32,
    pub control: f32,
    pub engage: f32,
    pub risk: f32,
    /// Share of room, margin (`wear`) and rubber's price (`rubber`) a heat
    /// takes off, starting at heat `wear_at` and full one heat later.
    pub wear: f32,
    pub rubber: f32,
    pub wear_at: f32,
}

/// The artist's figure rating.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artist {
    /// What the figure turned the other way costs in rating.
    pub mirror_cost: f32,
    /// A leg this much (units) off the stride still counts as on it.
    pub slack: f32,
}

/// Level of detail by load: with more riders alive, every bot plans less.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tiers {
    /// Riders alive from which `t1`, then `t2`, apply.
    pub t1_riders: usize,
    pub t2_riders: usize,
    pub full: Limits,
    pub t1: Limits,
    pub t2: Limits,
}

/// What one tier lets a bot plan.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Cells a room measurement may flood.
    pub flood_cap: u32,
    /// Longest horizon, in ticks.
    pub max_horizon: u32,
    /// Richest maneuver variety (see `BotKnobs::variety`).
    pub max_variety: u32,
}

impl Tiers {
    /// The tier for `alive` riders.
    pub fn of_load(&self, alive: usize) -> Tier {
        if alive >= self.t2_riders {
            Tier::T2
        } else if alive >= self.t1_riders {
            Tier::T1
        } else {
            Tier::Full
        }
    }

    pub fn limits(&self, tier: Tier) -> Limits {
        match tier {
            Tier::Full => self.full,
            Tier::T1 => self.t1,
            Tier::T2 => self.t2,
        }
    }
}
