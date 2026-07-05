//! Follow camera: setup + per-frame tracking.

use bevy::camera::ScalingMode;
use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use shared::{Player, PlayerInput, Position};

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

/// Hard-snap the camera to the local player's position each frame.
/// Smooth interpolation is planned for the polish phase.
pub fn follow_player(
    player: Query<&Position, (With<Player>, With<InputMarker<PlayerInput>>)>,
    mut camera: Query<&mut Transform, With<Camera2d>>,
) {
    let Ok(pos) = player.single() else {
        return;
    };
    let Ok(mut transform) = camera.single_mut() else {
        return;
    };
    transform.translation.x = pos.0.x;
    transform.translation.y = pos.0.y;
}
