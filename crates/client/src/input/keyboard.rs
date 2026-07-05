//! Keyboard → turn-input mapping.
//!
//! `buffer_keyboard_input` (Update) reads `just_pressed` every frame and stores
//! the turn in `PendingInput`. `read_keyboard` (FixedPreUpdate) consumes that
//! buffer into the local player's `ActionState<PlayerInput>` each sim tick.
//!
//! Decoupling avoids skipped turns on high-refresh-rate monitors where frames
//! with zero sim ticks would clear `just_pressed` before any tick can consume it.

use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use shared::ActionState;
use shared::PlayerInput;

#[derive(Resource, Default)]
pub struct PendingInput(pub Option<PlayerInput>);

pub fn buffer_keyboard_input(keys: Res<ButtonInput<KeyCode>>, mut pending: ResMut<PendingInput>) {
    if pending.0.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyA) {
        pending.0 = Some(PlayerInput::TurnLeft);
    } else if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD) {
        pending.0 = Some(PlayerInput::TurnRight);
    }
}

pub fn read_keyboard(
    mut pending: ResMut<PendingInput>,
    mut players: Query<&mut ActionState<PlayerInput>, With<InputMarker<PlayerInput>>>,
) {
    let Some(mut action) = players.iter_mut().next() else {
        return;
    };
    if action.0 != PlayerInput::None {
        return;
    }
    if let Some(input) = pending.0.take() {
        action.0 = input;
    }
}
