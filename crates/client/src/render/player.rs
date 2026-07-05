//! Lightcycle + trail rendering.

use bevy::prelude::*;
use shared::{Direction, IsAlive, Player, PlayerColor, Position, TrailPoint, TrailPointOrder};

/// Draw trail line segments for every player.
///
/// For each player: collects its `TrailPoint` children, sorts by `TrailPointOrder`,
/// draws segments between consecutive points, then the active segment from the
/// last trail point to the player's current position.
pub fn draw_trails(
    players: Query<(Entity, &Position, &PlayerColor, &IsAlive), With<Player>>,
    trail_points: Query<(&Position, &TrailPointOrder, &ChildOf), With<TrailPoint>>,
    mut gizmos: Gizmos,
) {
    for (player_entity, player_pos, color, alive) in &players {
        let trail_color = if alive.0 {
            Color::srgb_u32(color.0)
        } else {
            Color::srgb(0.3, 0.3, 0.3)
        };

        let mut points: Vec<(u32, Vec2)> = trail_points
            .iter()
            .filter(|(_, _, child_of)| child_of.0 == player_entity)
            .map(|(pos, order, _)| (order.0, pos.0))
            .collect();
        points.sort_by_key(|(order, _)| *order);

        if points.is_empty() {
            continue;
        }

        for window in points.windows(2) {
            gizmos.line_2d(window[0].1, window[1].1, trail_color);
        }

        let last = points.last().unwrap().1;
        gizmos.line_2d(last, player_pos.0, trail_color);
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
            Color::srgb(0.3, 0.3, 0.3)
        };
        gizmos.circle_2d(pos.0, 14.0, body).resolution(24);
        gizmos.line_2d(pos.0, pos.0 + dir.0 * 28.0, body);
    }
}
