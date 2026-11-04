//! Lightcycle + trail rendering.

use bevy::prelude::*;
use shared::{Direction, DisplayName, IsAlive, Player, PlayerColor};

use super::layers::Z_LABEL;
use super::smooth::Rendered;
use crate::theme::Theme;

/// Radius of the circle drawn for a lightcycle, in world units.
pub(super) const RIDER_RADIUS: f32 = 14.0;
/// Trail points closer than this to the previous one are dropped before
/// drawing: a zero-length segment gives a line joint no direction.
const MIN_SEGMENT: f32 = 0.01;
/// World-space Y offset of the name label above the rider.
const NAME_Y_OFFSET: f32 = 26.0;
/// World-space font size; small relative to the 14-unit rider radius.
const NAME_FONT_SIZE: f32 = 20.0;
/// Fallback label when `DisplayName` is missing or empty (pre-replication).
const FALLBACK_NAME: &str = "Player";

/// Local-only label entity tracking its rider.
#[derive(Component)]
pub struct NameLabel {
    rider: Entity,
}

/// Draw trail line segments for every player, from the smoothed render pose.
pub fn draw_trails(
    players: Query<(&Rendered, &PlayerColor, &IsAlive), With<Player>>,
    theme: Res<Theme>,
    mut gizmos: Gizmos,
) {
    for (rendered, color, alive) in &players {
        let trail_color = if alive.0 {
            Color::srgb_u32(color.0)
        } else {
            theme.dead_trail.bevy()
        };
        // One strip, not loose segments, so the corners get joints.
        gizmos.linestrip_2d(without_degenerate_segments(&rendered.trail), trail_color);
    }
}

/// The trail points, minus any that sit on top of the previous one (right
/// after a turn the head can coincide with the corner).
fn without_degenerate_segments(points: &[Vec2]) -> impl Iterator<Item = Vec2> + '_ {
    let mut last: Option<Vec2> = None;
    points.iter().copied().filter(move |&point| {
        let keep = last.is_none_or(|prev| prev.distance(point) >= MIN_SEGMENT);
        if keep {
            last = Some(point);
        }
        keep
    })
}

/// Draw every lightcycle: a filled circle plus a short heading line.
pub fn draw_players(
    players: Query<(&Rendered, &Direction, &PlayerColor, &IsAlive), With<Player>>,
    theme: Res<Theme>,
    mut gizmos: Gizmos,
) {
    for (rendered, dir, color, alive) in &players {
        let head = rendered.head;
        let body = if alive.0 {
            Color::srgb_u32(color.0)
        } else {
            theme.dead_rider.bevy()
        };
        gizmos.circle_2d(head, RIDER_RADIUS, body).resolution(24);
        if alive.0 {
            gizmos.line_2d(head, head + rendered.heading(dir.0) * 28.0, body);
        } else {
            gizmos.line_2d(head - Vec2::splat(18.0), head + Vec2::splat(18.0), body);
            gizmos.line_2d(
                head + Vec2::new(-18.0, 18.0),
                head + Vec2::new(18.0, -18.0),
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
            &Rendered,
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

    for (rider, name, rendered, color, alive) in &riders {
        let text = name.map_or(FALLBACK_NAME, |name| {
            if name.0.is_empty() {
                FALLBACK_NAME
            } else {
                name.0.as_str()
            }
        });
        let translation = Vec3::new(rendered.head.x, rendered.head.y + NAME_Y_OFFSET, Z_LABEL);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coincident_points_are_dropped_but_corners_stay() {
        let points = [
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 50.0),
            Vec2::new(100.0, 50.0),
        ];
        let kept: Vec<Vec2> = without_degenerate_segments(&points).collect();
        assert_eq!(
            kept,
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(100.0, 50.0)
            ]
        );
    }
}
