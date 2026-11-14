//! Tron Zero — shared simulation core.
//!
//! Pure bevy_ecs components + systems. No networking, no rendering.
//! Both client and server depend on this crate so the simulation stays
//! identical across peers.

pub mod components;
pub mod constants;
pub mod elimination;
pub mod match_rules;
pub mod math;
pub mod physics;
pub mod polyline;
pub mod probe;
pub mod protocol;
pub mod spawn;
pub mod systems;

// Re-export everything at the crate root for ergonomic access.
pub use components::arena::*;
pub use components::debug::*;
pub use components::player::*;
pub use components::round::*;
pub use components::trail::*;
pub use constants::*;
pub use elimination::*;
pub use lightyear::prelude::input::native::ActionState;
pub use match_rules::{EndRules, MatchFlow, MatchRules, MatchStep, Respawn, RoundEnd};
pub use math::*;
pub use physics::*;
pub use systems::player::*;
