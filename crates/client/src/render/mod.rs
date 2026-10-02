pub mod arena;
pub mod camera;
pub mod death;
pub mod hud;
pub mod player;

pub use arena::draw_arena;
pub use camera::{follow_player, setup_camera};
pub use death::death_ui;
pub use hud::hud_ui;
pub use player::{draw_players, draw_trails};
