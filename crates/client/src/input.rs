//! Keyboard → turn-input mapping.
//!
//! Reads physical key locations (QWERTY-agnostic). Left = turn left,
//! Right = turn right. A pending input is consumed by the next fixed tick's
//! `apply_turn`; while one is pending we drop further presses so a single key
//! tap produces a single 90° turn.

use bevy::prelude::*;
use shared::PlayerInput;
use shared::PendingInput;

pub fn read_keyboard(keys: Res<ButtonInput<KeyCode>>, mut pending: ResMut<PendingInput>) {
    // Don't overwrite an input the simulation hasn't consumed yet.
    if pending.0 != PlayerInput::None {
        return;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyA) {
        pending.0 = PlayerInput::TurnLeft;
    } else if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD) {
        pending.0 = PlayerInput::TurnRight;
    }
}
