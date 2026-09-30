//! Follow camera: setup + per-frame tracking.

use bevy::camera::ScalingMode;
use bevy::prelude::*;
use lightyear::prelude::Predicted;
use lightyear::prelude::input::native::InputMarker;
use shared::{Player, PlayerInput, Position};

/// Vertical world-units visible in the follow camera.
const CAMERA_VIEW_HEIGHT: f32 = 800.0;
/// Camera distance from the 2D plane.
const CAMERA_Z: f32 = 999.0;

type LocalPlayer = (
    With<Player>,
    With<InputMarker<PlayerInput>>,
    With<Predicted>,
);

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
    player: Query<&Position, LocalPlayer>,
    mut camera: Query<&mut Transform, With<Camera2d>>,
) {
    if player.is_empty() {
        for mut transform in &mut camera {
            transform.translation.x = 0.0;
            transform.translation.y = 0.0;
        }
    }
    for pos in &player {
        for mut transform in &mut camera {
            transform.translation.x = pos.0.x;
            transform.translation.y = pos.0.y;
        }
    }
}
