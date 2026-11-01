//! Decision scheduling and graceful degradation.

use bevy::prelude::*;

const T1_RIDERS: usize = 5;
const T2_RIDERS: usize = 8;

#[derive(Resource, Clone, Copy, Debug)]
pub struct SchedulerConfig {
    /// `false` decides every tick (tests/tuning); `true` staggers by epoch.
    pub enabled: bool,
    /// Non-zero overrides every bot's reaction-based epoch.
    pub base_epoch_ticks: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_epoch_ticks: 0,
        }
    }
}

/// Level-of-detail tier. Degradation may shrink sensing work but never physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Full,
    T1,
    T2,
}

impl Tier {
    pub fn from_load(active_riders: usize, _config: &SchedulerConfig) -> Self {
        if active_riders >= T2_RIDERS {
            Tier::T2
        } else if active_riders >= T1_RIDERS {
            Tier::T1
        } else {
            Tier::Full
        }
    }

    pub fn max_bfs(self) -> u32 {
        match self {
            Tier::Full => 800,
            Tier::T1 => 400,
            Tier::T2 => 200,
        }
    }

    pub fn max_lookahead(self) -> u32 {
        match self {
            Tier::Full => 16,
            Tier::T1 => 8,
            Tier::T2 => 4,
        }
    }
}

/// The single redecide predicate. `enabled=false` decides every tick; otherwise
/// the global override wins, else each bot's reaction period phase-shifts by
/// its slot so expensive work is spread across ticks.
pub fn should_redecide(
    config: &SchedulerConfig,
    tick: u32,
    slot_index: usize,
    reaction_ticks: u32,
) -> bool {
    if !config.enabled {
        return true;
    }
    let epoch = if config.base_epoch_ticks > 0 {
        config.base_epoch_ticks
    } else {
        reaction_ticks.max(1)
    };
    let epoch = epoch.max(1);
    tick % epoch == (slot_index as u32) % epoch
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Five-bot roster: small games stay Full LOD, degradation kicks in at
    /// five riders (T1) and eight riders (T2).
    #[test]
    fn tier_thresholds_fit_five_bot_roster() {
        let config = SchedulerConfig::default();
        assert_eq!(Tier::from_load(1, &config), Tier::Full);
        assert_eq!(Tier::from_load(4, &config), Tier::Full);
        assert_eq!(Tier::from_load(5, &config), Tier::T1);
        assert_eq!(Tier::from_load(7, &config), Tier::T1);
        assert_eq!(Tier::from_load(8, &config), Tier::T2);
    }
}
