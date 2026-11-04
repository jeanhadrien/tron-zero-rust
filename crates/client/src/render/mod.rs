pub mod arena;
pub mod camera;
pub mod death;
pub mod debug;
pub mod hud;
pub mod player;
pub mod smooth;

pub use arena::draw_arena;
pub use camera::{follow_player, setup_camera};
pub use death::death_ui;
pub use debug::{DebugOverlay, debug_panel_ui, draw_bot_debug};
pub use hud::{hud_ui, status_badge_ui};
pub use player::{draw_players, draw_trails, sync_name_labels};
pub use smooth::{RenderSet, add_smoothing};
