//! Arena boundary wall + faint grid rendering.
//!
//! The grid is always drawn (connected or not) so the overlay's 32x32 cell
//! highlights have a visible reference. Colors and line width come from the
//! active [`Theme`]; the geometry tunables live at the top.
//!
//! The grid is made of sprites at a negative z: gizmos (trails, riders, walls)
//! ignore depth in 2D and are always sorted after sprites, so the grid can
//! only stay behind them if it is not a gizmo itself.

use bevy::gizmos::config::{DefaultGizmoConfigGroup, GizmoConfigStore, GizmoLineJoint};
use bevy::prelude::*;
use shared::{ARENA_HEIGHT, ARENA_WIDTH, Arena, WallSegments};

use super::layers::Z_GRID;
use crate::theme::Theme;

/// Grid line spacing in world units.
pub const GRID_STEP: f32 = 100.0;
/// Grid line thickness in world units.
pub const GRID_THICKNESS: f32 = 1.5;

/// Marks a grid line sprite so a theme change can recolor it.
#[derive(Component)]
pub struct GridLine;

/// Spawn the grid as thin sprites covering the arena.
pub fn spawn_grid(mut commands: Commands, theme: Res<Theme>) {
    let (half_w, half_h) = (ARENA_WIDTH * 0.5, ARENA_HEIGHT * 0.5);
    let color = theme.grid.bevy();
    let line = |center: Vec2, size: Vec2| {
        (
            GridLine,
            Sprite::from_color(color, size),
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

/// Push the theme's world tokens to the renderer: clear color, grid color and
/// gizmo line style. Runs when the theme changes (and once at startup, since a
/// freshly inserted resource counts as changed).
pub fn apply_world_theme(
    theme: Res<Theme>,
    mut clear: ResMut<ClearColor>,
    mut grid: Query<&mut Sprite, With<GridLine>>,
    mut gizmo_config: ResMut<GizmoConfigStore>,
) {
    if !theme.is_changed() {
        return;
    }
    clear.0 = theme.background.bevy();
    let color = theme.grid.bevy();
    for mut sprite in &mut grid {
        sprite.color = color;
    }
    let (config, _) = gizmo_config.config_mut::<DefaultGizmoConfigGroup>();
    config.line.width = theme.line_width;
    // Sharp corners where a trail turns, however thick the line.
    config.line.joints = GizmoLineJoint::Miter;
}

/// Draw the boundary walls when replicated state is present.
pub fn draw_arena(walls: Query<&WallSegments, With<Arena>>, theme: Res<Theme>, mut gizmos: Gizmos) {
    let Ok(segments) = walls.single() else {
        return;
    };
    for seg in &segments.0 {
        gizmos.line_2d(
            Vec2::new(seg[0], seg[1]),
            Vec2::new(seg[2], seg[3]),
            theme.wall.bevy(),
        );
    }
}
