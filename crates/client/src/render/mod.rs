pub mod arena;
pub mod camera;
pub mod player;

pub use arena::draw_arena;
pub use camera::{follow_player, setup_camera};
pub use player::{draw_players, draw_trails};
