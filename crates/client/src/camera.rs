//! Follow camera.
//!
//! MVP: hard-snap the camera to the local player's position each frame. Smooth
//! interpolation is planned for the polish phase.

use bevy::prelude::*;
use shared::{Player, Position};

pub fn follow_player(
    player: Query<&Position, With<Player>>,
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
    // z is left as initialised in `setup_camera`.
}
