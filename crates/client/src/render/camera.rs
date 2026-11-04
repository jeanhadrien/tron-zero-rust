//! Follow camera: setup + per-frame tracking.

use bevy::camera::ScalingMode;
use bevy::prelude::*;

use super::layers::{CAMERA_FAR, CAMERA_Z};
use super::smooth::{OwnRider, Rendered};

/// Vertical world-units visible in the follow camera.
const CAMERA_VIEW_HEIGHT: f32 = 800.0;
/// Duration of the camera glide after the rider respawns somewhere else.
const RESPAWN_EASE_SECS: f32 = 0.2;

/// Spawn a 2D orthographic follow camera centred on the origin.
pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: CAMERA_VIEW_HEIGHT,
            },
            far: CAMERA_FAR,
            ..OrthographicProjection::default_2d()
        }),
        Transform::from_xyz(0.0, 0.0, CAMERA_Z),
    ));
}

/// Glide in progress after a respawn teleport.
#[derive(Default)]
pub struct RespawnEase {
    from: Vec2,
    left: f32,
}

/// Lock the camera to the local rider's render head (the same value the body
/// is drawn at, so it never jitters against it). Only a respawn teleport is
/// eased; death, resync and first sight snap.
pub fn follow_player(
    time: Res<Time>,
    player: Query<(&Rendered, &OwnRider)>,
    mut camera: Query<&mut Transform, With<Camera2d>>,
    mut ease: Local<RespawnEase>,
) {
    let (target, teleported) = match player.single() {
        Ok((rendered, own)) => (rendered.head, own.teleported()),
        Err(_) => (Vec2::ZERO, false),
    };
    for mut transform in &mut camera {
        let shown = transform.translation.truncate();
        if teleported {
            *ease = RespawnEase {
                from: shown,
                left: RESPAWN_EASE_SECS,
            };
        }
        let at = if ease.left > 0.0 {
            ease.left = (ease.left - time.delta_secs()).max(0.0);
            let progress = 1.0 - ease.left / RESPAWN_EASE_SECS;
            ease.from.lerp(target, smoothstep(progress))
        } else {
            target
        };
        transform.translation.x = at.x;
        transform.translation.y = at.y;
    }
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
