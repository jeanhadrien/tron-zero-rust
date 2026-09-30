//! Arena boundary wall rendering.

use bevy::prelude::*;
use shared::{Arena, WallSegments};

/// Draw the arena boundary walls.
pub fn draw_arena(walls: Query<&WallSegments, With<Arena>>, mut gizmos: Gizmos) {
    let Ok(segments) = walls.single() else {
        // Disconnected backdrop only; never spawn fake replicated arena state.
        for coordinate in (-1200..=1200).step_by(100) {
            let p = coordinate as f32;
            let color = Color::srgb(0.06, 0.12, 0.15);
            gizmos.line_2d(Vec2::new(p, -1200.0), Vec2::new(p, 1200.0), color);
            gizmos.line_2d(Vec2::new(-1200.0, p), Vec2::new(1200.0, p), color);
        }
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
