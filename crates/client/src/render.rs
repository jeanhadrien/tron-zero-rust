//! Gizmo-based renderer.
//!
//! MVP rendering: arena boundary walls, trail line segments, and each
//! lightcycle as a circle with a heading line. Drawn every frame in `Update`
//! so gizmos clear per render frame.

use bevy::camera::ScalingMode;
use bevy::prelude::*;
use shared::{
    Arena, Direction, IsAlive, Player, PlayerColor, Position, TrailPoint, TrailPointOrder,
    WallSegments,
};

/// Vertical world-units visible in the follow camera.
const CAMERA_VIEW_HEIGHT: f32 = 800.0;
/// Camera distance from the 2D plane.
const CAMERA_Z: f32 = 999.0;

/// Spawn a 2D orthographic follow camera centred on the origin.
pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: CAMERA_VIEW_HEIGHT,
            },
            ..OrthographicProjection::default_2d()
        }),
        Transform::from_xyz(0.0, 0.0, CAMERA_Z),
    ));
}

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

        // Collect this player's trail points (children filtered by ChildOf target).
        let mut points: Vec<(u32, Vec2)> = trail_points
            .iter()
            .filter(|(_, _, child_of)| child_of.0 == player_entity)
            .map(|(pos, order, _)| (order.0, pos.0))
            .collect();
        points.sort_by_key(|(order, _)| *order);

        if points.is_empty() {
            continue;
        }

        // Segments between consecutive trail points.
        for window in points.windows(2) {
            gizmos.line_2d(window[0].1, window[1].1, trail_color);
        }

        // Active segment: last trail point → current player position.
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
        // Cycle body.
        gizmos.circle_2d(pos.0, 14.0, body).resolution(24);
        // Heading indicator (length scaled to be visible against the body).
        gizmos.line_2d(pos.0, pos.0 + dir.0 * 28.0, body);
    }
}
