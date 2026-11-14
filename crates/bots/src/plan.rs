//! Actions, maneuvers (the planner's candidates) and the plan a brain follows.

use shared::PlayerInput;

/// One tick's steering input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Hold,
    Left,
    Right,
}

impl Action {
    pub fn to_input(self) -> PlayerInput {
        match self {
            Action::Hold => PlayerInput::None,
            Action::Left => PlayerInput::TurnLeft,
            Action::Right => PlayerInput::TurnRight,
        }
    }

    /// Stable id for the `BotDebug` snapshot (Hold = 0, Left = 1, Right = 2).
    pub fn index(self) -> usize {
        match self {
            Action::Hold => 0,
            Action::Left => 1,
            Action::Right => 2,
        }
    }
}

/// Shape of a maneuver, for debugging and for the commitment bonus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hold,
    /// One turn, then straight on.
    Turn,
    /// Turn, then turn back the other way: a sideways step.
    Dodge,
    /// Two turns the same way: reverse along a parallel lane.
    UTurn,
    /// What is left of the plan being followed.
    Continue,
}

/// A maneuver has at most this many turns.
pub const MAX_TURNS: usize = 2;

/// A turn `at` ticks after the maneuver starts (0 = on its first tick).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    pub at: u32,
    pub action: Action,
}

/// A candidate: a few turns at given offsets, then straight on. Turns are
/// sorted by offset and sit on different ticks (the sim takes one per tick).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Maneuver {
    pub kind: Kind,
    turns: [Turn; MAX_TURNS],
    len: usize,
}

impl Maneuver {
    pub fn hold() -> Self {
        Self::new(Kind::Hold, &[])
    }

    pub fn new(kind: Kind, turns: &[Turn]) -> Self {
        debug_assert!(turns.len() <= MAX_TURNS);
        debug_assert!(turns.windows(2).all(|pair| pair[0].at < pair[1].at));
        let mut stored = [Turn {
            at: 0,
            action: Action::Hold,
        }; MAX_TURNS];
        stored[..turns.len()].copy_from_slice(turns);
        Self {
            kind,
            turns: stored,
            len: turns.len(),
        }
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns[..self.len]
    }

    /// The steering input `tick` ticks after the start.
    pub fn action_at(&self, tick: u32) -> Action {
        self.turns()
            .iter()
            .find(|turn| turn.at == tick)
            .map_or(Action::Hold, |turn| turn.action)
    }

    /// Offset of the last turn (0 when there is none).
    pub fn span(&self) -> u32 {
        self.turns().last().map_or(0, |turn| turn.at)
    }

    /// Action of the first turn when it is due right away, else `Hold`.
    pub fn first_action(&self) -> Action {
        self.action_at(0)
    }
}

/// The turns a brain still has to emit, on absolute ticks.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    turns: Vec<(u32, Action)>,
}

impl Plan {
    /// A plan that starts `maneuver` on `tick`.
    pub fn start(tick: u32, maneuver: &Maneuver) -> Self {
        Self {
            turns: maneuver
                .turns()
                .iter()
                .map(|turn| (tick + turn.at, turn.action))
                .collect(),
        }
    }

    /// Forget turns whose tick has passed without being emitted.
    pub fn drop_before(&mut self, tick: u32) {
        self.turns.retain(|&(at, _)| at >= tick);
    }

    /// A turn is due on `tick`.
    pub fn due(&self, tick: u32) -> bool {
        self.turns.iter().any(|&(at, _)| at == tick)
    }

    /// Remove and return the action for `tick`.
    pub fn take(&mut self, tick: u32) -> Action {
        match self.turns.iter().position(|&(at, _)| at == tick) {
            Some(index) => self.turns.remove(index).1,
            None => Action::Hold,
        }
    }

    /// What is left, as a maneuver starting on `tick`. `None` when empty.
    pub fn remaining(&self, tick: u32) -> Option<Maneuver> {
        let turns: Vec<Turn> = self
            .turns
            .iter()
            .filter(|&&(at, _)| at >= tick)
            .map(|&(at, action)| Turn {
                at: at - tick,
                action,
            })
            .collect();
        (!turns.is_empty()).then(|| Maneuver::new(Kind::Continue, &turns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Action = Action::Left;

    fn u_turn(gap: u32) -> Maneuver {
        Maneuver::new(
            Kind::UTurn,
            &[
                Turn {
                    at: 3,
                    action: LEFT,
                },
                Turn {
                    at: 3 + gap,
                    action: LEFT,
                },
            ],
        )
    }

    #[test]
    fn a_plan_emits_each_turn_on_its_own_tick() {
        let mut plan = Plan::start(100, &u_turn(5));
        let emitted: Vec<(u32, Action)> = (100..115).map(|t| (t, plan.take(t))).collect();
        let turns: Vec<u32> = emitted
            .iter()
            .filter(|(_, action)| *action != Action::Hold)
            .map(|(tick, _)| *tick)
            .collect();
        assert_eq!(turns, [103, 108]);
    }

    #[test]
    fn a_turn_at_offset_zero_is_emitted_on_the_planning_tick() {
        let maneuver = Maneuver::new(
            Kind::Turn,
            &[Turn {
                at: 0,
                action: LEFT,
            }],
        );
        let mut plan = Plan::start(100, &maneuver);
        assert!(plan.due(100));
        assert_eq!(plan.take(100), LEFT);
    }

    #[test]
    fn the_rest_of_a_plan_is_a_maneuver_from_now() {
        let mut plan = Plan::start(100, &u_turn(5));
        assert_eq!(plan.take(103), LEFT);
        let rest = plan.remaining(104).unwrap();
        assert_eq!(rest.kind, Kind::Continue);
        assert_eq!(rest.turns().len(), 1);
        assert_eq!(rest.turns()[0].at, 4);
        plan.drop_before(109);
        assert!(plan.remaining(109).is_none());
    }
}
