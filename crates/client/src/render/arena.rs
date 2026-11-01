//! Arena boundary wall + faint grid rendering.
//!
//! The grid is always drawn (connected or not) so the overlay's 32x32 cell
//! highlights have a visible reference. Tunables live at the top.

use bevy::prelude::*;
use shared::{ARENA_HEIGHT, ARENA_WIDTH, Arena, WallSegments};

/// Grid line spacing in world units.
pub const GRID_STEP: f32 = 100.0;
/// Faint grid line color (tune here).
pub const GRID_COLOR: Color = Color::srgb(0.06, 0.12, 0.15);
/// Arena boundary wall color (tune here).
pub const WALL_COLOR: Color = Color::srgb(0.45, 0.5, 0.55);

/// Draw the faint grid across the whole arena plus the boundary walls when
/// replicated state is present.
pub fn draw_arena(walls: Query<&WallSegments, With<Arena>>, mut gizmos: Gizmos) {
    let (half_w, half_h) = match walls.single() {
        Ok(segments) => {
            draw_grid(&mut gizmos, ARENA_WIDTH * 0.5, ARENA_HEIGHT * 0.5);
            for seg in &segments.0 {
                gizmos.line_2d(
                    Vec2::new(seg[0], seg[1]),
                    Vec2::new(seg[2], seg[3]),
                    WALL_COLOR,
                );
            }
            return;
        }
        Err(_) => (1200.0, 1200.0),
    };
    // Disconnected backdrop only; never spawn fake replicated arena state.
    draw_grid(&mut gizmos, half_w, half_h);
}

/// Axis-aligned grid lines over `[-half_w, half_w] x [-half_h, half_h]`.
fn draw_grid(gizmos: &mut Gizmos, half_w: f32, half_h: f32) {
    let mut x = -half_w;
    while x <= half_w + 0.01 {
        gizmos.line_2d(Vec2::new(x, -half_h), Vec2::new(x, half_h), GRID_COLOR);
        x += GRID_STEP;
    }
    let mut y = -half_h;
    while y <= half_h + 0.01 {
        gizmos.line_2d(Vec2::new(-half_w, y), Vec2::new(half_w, y), GRID_COLOR);
        y += GRID_STEP;
    }
}
