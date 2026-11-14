//! The objective a bot maximises: a weighted sum of named terms.
//!
//! Personality is the weight vector (see `Personality::weights`); difficulty is
//! the budget (`BotKnobs`). A new goal is one field in [`Terms`], one in
//! [`Weights`], one entry in each `values` array and one line in the planner
//! that measures it. Every term is "higher is better": losses are negative.

/// How many terms there are; the length of both `values` arrays.
pub const TERM_COUNT: usize = 7;

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
    };

    pub fn values(&self) -> [f32; TERM_COUNT] {
        [
            self.space,
            self.pocket,
            self.reach,
            self.clear,
            self.turn,
            self.fill,
            self.commit,
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
