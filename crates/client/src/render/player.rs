//! Lightcycle + trail rendering.

use bevy::prelude::*;
use shared::{Direction, DisplayName, IsAlive, Player, PlayerColor, Position, Trail};

/// World-space Y offset of the name label above the rider.
const NAME_Y_OFFSET: f32 = 26.0;
/// World-space font size; small relative to the 14-unit rider radius.
const NAME_FONT_SIZE: f32 = 20.0;
/// Fallback label when `DisplayName` is missing or empty (pre-replication).
const FALLBACK_NAME: &str = "Player";

/// Local-only label entity tracking its rider.
#[derive(Component)]
pub(crate) struct NameLabel {
    rider: Entity,
}

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

/// Spawn/update one world-space `Text2d` label per rider.
///
/// Local-only: labels are never replicated. Text comes from the replicated
/// `DisplayName` (server-set; `"Player"` for humans, `{strategy}-{elo} #{N}`
/// for bots) with a hardcoded `"Player"` fallback while it is missing.
#[allow(clippy::type_complexity)]
pub fn sync_name_labels(
    mut commands: Commands,
    riders: Query<
        (
            Entity,
            Option<&DisplayName>,
            &Position,
            &PlayerColor,
            &IsAlive,
        ),
        With<Player>,
    >,
    mut labels: Query<(
        Entity,
        &NameLabel,
        &mut Transform,
        &mut Text2d,
        &mut TextColor,
        &mut Visibility,
    )>,
) {
    // Index existing labels by rider.
    let mut by_rider = bevy::platform::collections::HashMap::new();
    for (entity, label, _, _, _, _) in &labels {
        by_rider.insert(label.rider, entity);
    }

    for (rider, name, pos, color, alive) in &riders {
        let text = name.map_or(FALLBACK_NAME, |name| {
            if name.0.is_empty() {
                FALLBACK_NAME
            } else {
                name.0.as_str()
            }
        });
        let translation = Vec3::new(pos.0.x, pos.0.y + NAME_Y_OFFSET, 10.0);
        let tint = TextColor(Color::srgb_u32(color.0));
        let visibility = if alive.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

        if let Some(label_entity) = by_rider.remove(&rider) {
            let Ok((_, _, mut transform, mut text2d, mut text_color, mut vis)) =
                labels.get_mut(label_entity)
            else {
                continue;
            };
            if text2d.0 != text {
                text2d.0 = text.to_string();
            }
            transform.translation = translation;
            *text_color = tint;
            *vis = visibility;
        } else {
            commands.spawn((
                NameLabel { rider },
                Text2d::new(text),
                TextFont {
                    font_size: FontSize::Px(NAME_FONT_SIZE),
                    ..default()
                },
                tint,
                Transform::from_translation(translation),
                visibility,
            ));
        }
    }

    // Despawn labels whose rider is gone (disconnect / bot cycle).
    for (_, leftover) in by_rider {
        commands.entity(leftover).despawn();
    }
}
