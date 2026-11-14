//! The objective a bot maximises: a weighted sum of named terms.
//!
//! Personality is the weight vector (see `Personality::weights`); difficulty is
//! the budget (`BotKnobs`). A new goal is one field in [`Terms`], one in
//! [`Weights`], one entry in each `values` array and one line in the planner
//! that measures it. Every term is "higher is better": losses are negative.

use serde::{Deserialize, Serialize};

/// How many terms there are; the length of both `values` arrays.
pub const TERM_COUNT: usize = 13;

/// What a full round heat adds to the fight terms and to the risk a bot takes
/// (the same for every personality: a round has to end), and takes off room,
/// margin and rubber's price (share of the weight).
const HEAT_PRESSURE: f32 = 5.0;
const HEAT_CONTROL: f32 = 0.5;
const HEAT_ENGAGE: f32 = 3.0;
const HEAT_RISK: f32 = 2.0;
const HEAT_WEAR: f32 = 0.8;
const HEAT_RUBBER: f32 = 0.8;
/// Heat at which wear and rubber's price start to go (full one heat later):
/// the fight comes first (kills come late in a round, once the heat is up), a
/// round nobody wins is only forced to an end afterwards.
const WEAR_AT: f32 = 2.0;

/// What a candidate maneuver achieves, one measurement per term.
#[derive(Clone, Copy, Debug, Default)]
pub struct Terms {
    /// Room to ride in from where the maneuver ends, `0..=1` (log of cells).
    pub space: f32,
    /// Missing room when the region is smaller than the bot needs, `-1..=0`.
    pub pocket: f32,
    /// Share of the horizon ridden before an obstacle stops the line, `0..=1`.
    pub reach: f32,
    /// Free run ahead of the end pose, `0..=1`.
    pub clear: f32,
    /// Turns spent (each costs speed and invites dithering), `-1..=0`.
    pub turn: f32,
    /// In a small region: how much of the new path runs along something closed
    /// (a wall, a trail, itself), so the space is filled without gaps, `0..=1`.
    pub fill: f32,
    /// 1 for the plan already being followed.
    pub commit: f32,
    /// Kill pressure: how little room the target keeps (its share of the
    /// arena it reaches first, small is high), plus a bonus when it is sealed
    /// in a pocket or has no survivable reply, `0..=2`.
    pub pressure: f32,
    /// Territory control: share of the claimed arena the bot reaches before
    /// every opponent, `0..=1`.
    pub control: f32,
    /// Engagement: how far the bot's heading keeps it from every opponent,
    /// `-1..=0`, scaled up as the round goes on.
    pub engage: f32,
    /// Speed earned by grinding: the line's average cruise speed against the
    /// bot's now, in multiples of the speed range, `-1..=1`.
    pub grind: f32,
    /// Rubber the line burns against obstacles ahead, as a share of what the
    /// bot has left, `-1..=0`: the more it is spent, the less is left for later.
    pub rubber: f32,
    /// How well the line continues the bot's figure, `-1..=1` (see `style`).
    pub style: f32,
}

impl Terms {
    pub fn values(&self) -> [f32; TERM_COUNT] {
        [
            self.space,
            self.pocket,
            self.reach,
            self.clear,
            self.turn,
            self.fill,
            self.commit,
            self.pressure,
            self.control,
            self.engage,
            self.grind,
            self.rubber,
            self.style,
        ]
    }
}

fn default_stride() -> f32 {
    200.0
}

/// How much a bot cares about each term. Same order as [`Terms::values`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub space: f32,
    pub pocket: f32,
    pub reach: f32,
    pub clear: f32,
    pub turn: f32,
    pub fill: f32,
    pub commit: f32,
    pub pressure: f32,
    pub control: f32,
    pub engage: f32,
    pub grind: f32,
    /// How dear rubber is: low means the bot spends it freely (late turns,
    /// tight lines), high means it keeps it for emergencies.
    pub rubber: f32,
    /// How much it cares to ride its figure (`style`); fighters keep it at 0.
    #[serde(default)]
    pub style: f32,
    /// Not a term: which figure it rides, an index into `style`'s cycles
    /// (0 staircase, 1 square wave).
    #[serde(default)]
    pub motif: f32,
    /// Not a term: the length (units) of every leg of the figure.
    #[serde(default = "default_stride")]
    pub stride: f32,
    /// Not a term: how much of the survival score a bot gives up to fight.
    /// The fight terms only choose among lines at most this far below the
    /// best by the survival terms, so they never pay for a death.
    pub risk: f32,
}

impl Weights {
    /// The shared starting point of every personality.
    pub const BASE: Weights = Weights {
        space: 1.0,
        pocket: 2.0,
        reach: 3.0,
        clear: 0.8,
        turn: 0.15,
        fill: 0.5,
        commit: 0.2,
        pressure: 1.5,
        control: 0.3,
        engage: 0.5,
        grind: 4.0,
        rubber: 0.5,
        style: 0.0,
        motif: 0.0,
        stride: 200.0,
        risk: 0.25,
    };

    /// The same bot without will to fight: no pressure, control, engagement
    /// or speed hunt.
    pub fn passive(self) -> Self {
        Self {
            pressure: 0.0,
            control: 0.0,
            engage: 0.0,
            grind: 0.0,
            ..self
        }
    }

    /// The same bot with a round `heat` (`0..=1`) old: it cares less about
    /// room and margin, wants the fight (and the risk that goes with it) more,
    /// and spends rubber freely, so a round whose riders all play safe still
    /// comes to an end.
    pub fn heated(self, heat: f32) -> Self {
        let late = (heat - WEAR_AT).clamp(0.0, 1.0);
        let worn = 1.0 - HEAT_WEAR * late;
        Self {
            space: self.space * worn,
            pocket: self.pocket * worn,
            clear: self.clear * worn,
            pressure: self.pressure + HEAT_PRESSURE * heat,
            control: self.control + HEAT_CONTROL * heat,
            engage: self.engage + HEAT_ENGAGE * heat,
            rubber: self.rubber * (1.0 - HEAT_RUBBER * late),
            risk: self.risk + HEAT_RISK * heat,
            ..self
        }
    }

    /// Whether any fight term is on.
    pub fn fights(&self) -> bool {
        self.pressure > 0.0 || self.control > 0.0 || self.engage > 0.0
    }

    /// Whether any term needs the territory race (the costly measurement).
    pub fn races(&self) -> bool {
        self.pressure > 0.0 || self.control > 0.0
    }

    pub fn values(&self) -> [f32; TERM_COUNT] {
        [
            self.space,
            self.pocket,
            self.reach,
            self.clear,
            self.turn,
            self.fill,
            self.commit,
            self.pressure,
            self.control,
            self.engage,
            self.grind,
            self.rubber,
            self.style,
        ]
    }

    /// The objective value of `terms`.
    pub fn value(&self, terms: &Terms) -> f32 {
        self.values()
            .iter()
            .zip(terms.values())
            .map(|(weight, term)| weight * term)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_is_the_weighted_sum() {
        let terms = Terms {
            space: 0.5,
            reach: 1.0,
            turn: -1.0,
            ..Terms::default()
        };
        let weights = Weights::BASE;
        let expected = 0.5 * weights.space + weights.reach - weights.turn;
        assert!((weights.value(&terms) - expected).abs() < 1e-6);
    }
}
