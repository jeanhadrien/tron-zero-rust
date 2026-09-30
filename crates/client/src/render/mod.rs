pub mod arena;
pub mod camera;
pub mod death;
pub mod hud;
pub mod player;

pub use arena::draw_arena;
pub use camera::{follow_player, setup_camera};
pub use death::{setup_death_overlay, update_death_overlay};
pub use hud::{setup_hud, update_hud};
pub use player::{draw_players, draw_trails};
