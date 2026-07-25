//! Keyboard → turn-input mapping.
//!
//! `buffer_keyboard_input` (Update) reads raw `KeyboardInput` messages directly
//! — each physical key press is one event — and pushes each key's turn into a
//! queue, spread over subsequent ticks. `read_keyboard` (FixedPreUpdate,
//! WriteClientInputs) always writes the current tick's input — from the queue
//! if non-empty, else `None` — following the lightyear continuous-input pattern
//! where ActionState is set every tick.

use std::collections::VecDeque;

use bevy::ecs::message::MessageReader;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use shared::ActionState;
use shared::PlayerInput;

const MAX_QUEUED_INPUTS: usize = 16;

#[derive(Resource, Default)]
pub struct PendingInput(pub VecDeque<PlayerInput>);

pub fn buffer_keyboard_input(
    mut key_events: MessageReader<KeyboardInput>,
    mut pending: ResMut<PendingInput>,
) {
    for event in key_events.read() {
        if !event.state.is_pressed() || event.repeat {
            continue;
        }
        if pending.0.len() >= MAX_QUEUED_INPUTS {
            break;
        }
        match event.key_code {
            KeyCode::ArrowLeft | KeyCode::KeyA => {
                pending.0.push_back(PlayerInput::TurnLeft);
            }
            KeyCode::ArrowRight | KeyCode::KeyD => {
                pending.0.push_back(PlayerInput::TurnRight);
            }
            _ => {}
        }
    }
}

pub fn read_keyboard(
    mut pending: ResMut<PendingInput>,
    mut players: Query<&mut ActionState<PlayerInput>, With<InputMarker<PlayerInput>>>,
) {
    let Some(mut action) = players.iter_mut().next() else {
        return;
    };
    // Always set the value for this tick so lightyear can buffer it and
    // apply_turn reads the correct input. None means "no turn this tick",
    // matching lightyear's continuous-input contract.
    let input = pending.0.pop_front().unwrap_or(PlayerInput::None);
    action.0 = input;
}
