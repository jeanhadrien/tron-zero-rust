//! Arena boundary wall rendering.

use bevy::prelude::*;
use shared::{Arena, WallSegments};

/// Draw the arena boundary walls.
pub fn draw_arena(walls: Query<&WallSegments, With<Arena>>, mut gizmos: Gizmos) {
    let Ok(segments) = walls.single() else {
        return;
    };
    let wall_color = Color::srgb(0.45, 0.5, 0.55);
    for seg in &segments.0 {
        gizmos.line_2d(
            Vec2::new(seg[0], seg[1]),
            Vec2::new(seg[2], seg[3]),
            wall_color,
        );
    }
}
