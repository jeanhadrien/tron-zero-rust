//! The objective a bot maximises: a weighted sum of named terms.
//!
//! Personality is the weight vector (see `Personality::weights`); difficulty is
//! the budget (`BotKnobs`). A new goal is one field in [`Terms`], one in
//! [`Weights`], one entry in each `values` array and one line in the planner
//! that measures it. Every term is "higher is better": losses are negative.

/// How many terms there are; the length of both `values` arrays.
pub const TERM_COUNT: usize = 10;

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
        ]
    }
}

/// How much a bot cares about each term. Same order as [`Terms::values`].
#[derive(Clone, Copy, Debug, PartialEq)]
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
        risk: 0.25,
    };

    /// The same bot without will to fight: no pressure, control or engagement.
    pub fn passive(self) -> Self {
        Self {
            pressure: 0.0,
            control: 0.0,
            engage: 0.0,
            ..self
        }
    }

    /// The same bot a while into the round: it cares less about room and
    /// margin (`k` in `0..=1`, `1` = not at all worn), so a round whose riders
    /// all play safe still comes to an end.
    pub fn worn(self, k: f32) -> Self {
        Self {
            space: self.space * k,
            pocket: self.pocket * k,
            clear: self.clear * k,
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
