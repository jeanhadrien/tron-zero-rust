//! Decision scheduling and graceful degradation.

use bevy_ecs::prelude::Resource;

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

/// Level-of-detail tier. Degradation may shrink planning work (flood size,
/// horizon, variety) but never physics. The thresholds and limits of each tier
/// are in the roster's tuning (`tuning::Tiers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Full,
    T1,
    T2,
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
