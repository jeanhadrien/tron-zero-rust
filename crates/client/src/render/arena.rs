//! Arena boundary wall + faint grid rendering.
//!
//! The grid is always drawn (connected or not) so the overlay's 32x32 cell
//! highlights have a visible reference. Tunables live at the top.
//!
//! The grid is made of sprites at a negative z: gizmos (trails, riders, walls)
//! ignore depth in 2D and are always sorted after sprites, so the grid can
//! only stay behind them if it is not a gizmo itself.

use bevy::prelude::*;
use shared::{ARENA_HEIGHT, ARENA_WIDTH, Arena, WallSegments};

use super::layers::Z_GRID;

/// Grid line spacing in world units.
pub const GRID_STEP: f32 = 100.0;
/// Grid line thickness in world units.
pub const GRID_THICKNESS: f32 = 1.5;
/// Faint grid line color (tune here).
pub const GRID_COLOR: Color = Color::srgb(0.06, 0.12, 0.15);
/// Arena boundary wall color (tune here).
pub const WALL_COLOR: Color = Color::srgb(0.45, 0.5, 0.55);

/// Spawn the grid as thin sprites covering the arena.
pub fn spawn_grid(mut commands: Commands) {
    let (half_w, half_h) = (ARENA_WIDTH * 0.5, ARENA_HEIGHT * 0.5);
    let line = |center: Vec2, size: Vec2| {
        (
            Sprite::from_color(GRID_COLOR, size),
            Transform::from_translation(center.extend(Z_GRID)),
        )
    };
    let mut x = -half_w;
    while x <= half_w + 0.01 {
        commands.spawn(line(
            Vec2::new(x, 0.0),
            Vec2::new(GRID_THICKNESS, ARENA_HEIGHT),
        ));
        x += GRID_STEP;
    }
    let mut y = -half_h;
    while y <= half_h + 0.01 {
        commands.spawn(line(
            Vec2::new(0.0, y),
            Vec2::new(ARENA_WIDTH, GRID_THICKNESS),
        ));
        y += GRID_STEP;
    }
}

/// Draw the boundary walls when replicated state is present.
pub fn draw_arena(walls: Query<&WallSegments, With<Arena>>, mut gizmos: Gizmos) {
    let Ok(segments) = walls.single() else {
        return;
    };
    for seg in &segments.0 {
        gizmos.line_2d(
            Vec2::new(seg[0], seg[1]),
            Vec2::new(seg[2], seg[3]),
            WALL_COLOR,
        );
    }
}
