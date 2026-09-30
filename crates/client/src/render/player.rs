//! Lightcycle + trail rendering.

use bevy::prelude::*;
use shared::{Direction, IsAlive, Player, PlayerColor, Position, Trail};

/// Draw trail line segments for every player.
///
/// Geometry, including the active head, comes from one replicated snapshot.
pub fn draw_trails(
    players: Query<(&Trail, &PlayerColor, &IsAlive), With<Player>>,
    mut gizmos: Gizmos,
) {
    for (trail, color, alive) in &players {
        let trail_color = if alive.0 {
            Color::srgb_u32(color.0)
        } else {
            Color::srgb(0.3, 0.3, 0.3)
        };

        for window in trail.0.windows(2) {
            gizmos.line_2d(window[0], window[1], trail_color);
        }
    }
}

/// Draw every lightcycle: a filled circle plus a short heading line.
pub fn draw_players(
    players: Query<(&Position, &Direction, &PlayerColor, &IsAlive), With<Player>>,
    mut gizmos: Gizmos,
) {
    for (pos, dir, color, alive) in &players {
        let body = if alive.0 {
            Color::srgb_u32(color.0)
        } else {
            Color::srgb(1.0, 0.15, 0.15)
        };
        gizmos.circle_2d(pos.0, 14.0, body).resolution(24);
        if alive.0 {
            gizmos.line_2d(pos.0, pos.0 + dir.0 * 28.0, body);
        } else {
            gizmos.line_2d(pos.0 - Vec2::splat(18.0), pos.0 + Vec2::splat(18.0), body);
            gizmos.line_2d(
                pos.0 + Vec2::new(-18.0, 18.0),
                pos.0 + Vec2::new(18.0, -18.0),
                body,
            );
        }
    }
}
