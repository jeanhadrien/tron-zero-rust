//! Tron Zero — shared simulation core.
//!
//! Pure bevy_ecs components + systems. No networking, no rendering.
//! Both client and server depend on this crate so the simulation stays
//! identical across peers.

pub mod components;
pub mod constants;
pub mod math;
pub mod protocol;
pub mod systems;

// Re-export everything at the crate root for ergonomic access.
pub use components::arena::*;
pub use components::player::*;
pub use components::trail::*;
pub use constants::*;
pub use lightyear::prelude::input::native::ActionState;
pub use math::*;
pub use systems::SimSet;
pub use systems::arena::*;
pub use systems::player::*;
