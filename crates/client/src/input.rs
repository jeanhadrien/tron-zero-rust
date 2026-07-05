//! Keyboard → turn-input mapping.
//!
//! Writes the current key press to the local player's `ActionState<PlayerInput>`
//! each fixed tick. Runs in lightyear's `WriteClientInputs` set so the input
//! is buffered for the current tick and replicated to the server.
//!
//! One key tap = one turn: Bevy's `just_pressed` clears after the first
//! `FixedPreUpdate` invocation in the frame, so catch-up ticks don't
//! generate spurious turns.

use bevy::prelude::*;
use shared::ActionState;
use shared::Player;
use shared::PlayerInput;

/// Write keyboard state to the local player's `ActionState` component.
/// Runs in `FixedPreUpdate::WriteClientInputs` — one invocation per sim tick.
pub fn read_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut players: Query<&mut ActionState<PlayerInput>, With<Player>>,
) {
    // MVP: single player. Pick the first (or only) player entity.
    let Some(mut action) = players.iter_mut().next() else {
        return;
    };

    // Don't overwrite an input the simulation hasn't processed yet.
    if action.0 != PlayerInput::None {
        return;
    }

    if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyA) {
        action.0 = PlayerInput::TurnLeft;
    } else if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD) {
        action.0 = PlayerInput::TurnRight;
    }
}
