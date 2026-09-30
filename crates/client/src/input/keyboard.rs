//! Keyboard → turn-input mapping.
//!
//! `buffer_keyboard_input` (PreUpdate) reads raw `KeyboardInput` messages directly
//! — each physical key press is one event — and pushes each key's turn into a
//! queue, spread over subsequent ticks. `read_keyboard` (FixedPreUpdate,
//! WriteClientInputs) always writes the current tick's input — from the queue
//! if non-empty, else `None` — following the lightyear continuous-input pattern
//! where ActionState is set every new tick. Rollback replays historical input
//! without consuming the live queue.

use std::collections::VecDeque;

use crate::menu::MenuState;
use bevy::ecs::message::MessageReader;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use lightyear::prelude::input::native::{InputMarker, NativeBuffer};
use lightyear::prelude::{
    Client, Connected, ControlledBy, InputTimeline, IsSynced, MessageReceiver, MessageSender,
    Rollback,
};
use shared::ActionState;
use shared::PlayerInput;
use shared::protocol::{RespawnChannel, RespawnOutcome, RespawnReply, RespawnRequest};
use shared::{IsAlive, LifeGeneration};

#[derive(Resource, Default)]
pub struct PendingInput(pub VecDeque<PlayerInput>);

#[derive(Resource, Default)]
pub struct InputLifecycle {
    observed: Option<(Entity, bool, Option<u128>)>,
}

impl InputLifecycle {
    fn observe(&mut self, state: Option<(Entity, bool, Option<u128>)>, pending: &mut PendingInput) {
        if self.observed != state || state.is_none_or(|(_, alive, _)| !alive) {
            pending.0.clear();
        }
        self.observed = state;
    }
}

#[derive(Resource, Default)]
pub struct RespawnUi {
    pub pending_generation: Option<u128>,
    pub outcome: Option<RespawnOutcome>,
    life: Option<(Entity, u128)>,
}

type InputReady = (
    With<InputMarker<PlayerInput>>,
    With<NativeBuffer<PlayerInput>>,
);

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn buffer_keyboard_input(
    mut key_events: MessageReader<KeyboardInput>,
    mut pending: ResMut<PendingInput>,
    mut lifecycle: ResMut<InputLifecycle>,
    mut respawn: ResMut<RespawnUi>,
    menu: Res<MenuState>,
    clients: Query<
        (Entity, Has<Rollback>),
        (With<Client>, With<Connected>, With<IsSynced<InputTimeline>>),
    >,
    players: Query<
        (
            Entity,
            &IsAlive,
            Option<&LifeGeneration>,
            Option<&ControlledBy>,
        ),
        InputReady,
    >,
    mut senders: Query<&mut MessageSender<RespawnRequest>>,
) {
    let client = clients.iter().next();
    // Rollback consumes historical ActionState, never the physical queue.
    let rollback = client.is_some_and(|(_, rollback)| rollback);
    let player = client.and_then(|(client_entity, _)| {
        players
            .iter()
            .find(|(_, _, _, controlled)| controlled.is_none_or(|c| c.owner == client_entity))
    });
    if !rollback {
        lifecycle.observe(
            player.map(|(entity, alive, life, _)| (entity, alive.0, life.map(|l| l.0))),
            &mut pending,
        );
        let current_life = player.and_then(|(entity, _, life, _)| life.map(|l| (entity, l.0)));
        if respawn.life != current_life {
            *respawn = RespawnUi {
                life: current_life,
                ..Default::default()
            };
        }
        if player.is_some_and(|(_, alive, _, _)| alive.0) {
            respawn.pending_generation = None;
            respawn.outcome = None;
        }
    }
    if menu.captures_input() {
        pending.0.clear();
        key_events.clear();
        return;
    }
    let can_turn = lifecycle.observed.is_some_and(|(_, alive, _)| alive);
    for event in key_events.read() {
        if !event.state.is_pressed() || event.repeat {
            continue;
        }
        let Some((_, alive, life, _)) = player else {
            continue;
        };
        match event.key_code {
            KeyCode::ArrowLeft | KeyCode::KeyA if can_turn => {
                pending.0.push_back(PlayerInput::TurnLeft);
            }
            KeyCode::ArrowRight | KeyCode::KeyD if can_turn => {
                pending.0.push_back(PlayerInput::TurnRight);
            }
            KeyCode::Space | KeyCode::Enter | KeyCode::NumpadEnter
                if !rollback && !alive.0 && respawn.pending_generation.is_none() =>
            {
                if let (Some((client_entity, _)), Some(life)) = (client, life)
                    && let Ok(mut sender) = senders.get_mut(client_entity)
                {
                    sender.send::<RespawnChannel>(RespawnRequest { generation: life.0 });
                    respawn.pending_generation = Some(life.0);
                    respawn.outcome = None;
                }
            }
            _ => {}
        }
    }
}

pub fn receive_respawn_replies(
    mut clients: Query<&mut MessageReceiver<RespawnReply>, (With<Client>, With<Connected>)>,
    mut respawn: ResMut<RespawnUi>,
) {
    for mut receiver in &mut clients {
        for reply in receiver.receive() {
            if respawn.pending_generation != Some(reply.generation) {
                continue;
            }
            respawn.outcome = Some(reply.outcome);
            // Accepted stays pending until the new authoritative life arrives.
            if reply.outcome != RespawnOutcome::Accepted {
                respawn.pending_generation = None;
            }
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn read_keyboard(
    client: Single<
        (Entity, &InputTimeline),
        (
            With<Client>,
            With<Connected>,
            With<IsSynced<InputTimeline>>,
            Without<Rollback>,
        ),
    >,
    mut pending: ResMut<PendingInput>,
    mut lifecycle: ResMut<InputLifecycle>,
    menu: Res<MenuState>,
    mut players: Query<
        (
            Entity,
            &mut ActionState<PlayerInput>,
            &IsAlive,
            Option<&LifeGeneration>,
            Option<&ControlledBy>,
        ),
        (
            With<InputMarker<PlayerInput>>,
            With<NativeBuffer<PlayerInput>>,
        ),
    >,
) {
    // Match Lightyear's buffering prerequisites: otherwise a press can be
    // consumed without ever entering the network input history.
    let (client_entity, _) = client.into_inner();
    let Some((entity, mut action, alive, life, _)) =
        players.iter_mut().find(|(_, _, _, _, controlled_by)| {
            controlled_by.is_none_or(|controlled_by| controlled_by.owner == client_entity)
        })
    else {
        return;
    };
    lifecycle.observe(Some((entity, alive.0, life.map(|l| l.0))), &mut pending);
    if menu.captures_input() {
        pending.0.clear();
    }
    // Always set the value for this tick so lightyear can buffer it and
    // apply_turn reads the correct input. None means "no turn this tick",
    // matching lightyear's continuous-input contract.
    let input = if alive.0 {
        pending.0.pop_front().unwrap_or(PlayerInput::None)
    } else {
        PlayerInput::None
    };
    action.0 = life.map_or(input, |life| input.for_life(life.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::{ButtonState, keyboard::Key};

    fn setup() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<PendingInput>();
        world.init_resource::<InputLifecycle>();
        world.init_resource::<RespawnUi>();
        world.insert_resource(MenuState {
            open: false,
            block_this_frame: false,
        });
        let client = world
            .spawn((
                Client::default(),
                lightyear::prelude::RemoteId(lightyear::prelude::PeerId::Server),
                Connected,
                InputTimeline::default(),
                IsSynced::<InputTimeline>::default(),
            ))
            .id();
        let player = world
            .spawn((
                ActionState::<PlayerInput>::default(),
                IsAlive(true),
                InputMarker::<PlayerInput>::default(),
                NativeBuffer::<PlayerInput>::default(),
            ))
            .id();
        world.resource_mut::<InputLifecycle>().observed = Some((player, true, None));
        (world, client, player)
    }

    #[test]
    fn same_frame_presses_preserve_order_without_truncation() {
        let (mut world, _, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        let keys = [
            KeyCode::ArrowLeft,
            KeyCode::KeyA,
            KeyCode::ArrowRight,
            KeyCode::KeyD,
        ];
        let expected = [
            PlayerInput::TurnLeft,
            PlayerInput::TurnLeft,
            PlayerInput::TurnRight,
            PlayerInput::TurnRight,
        ];
        for key in keys.into_iter().cycle().take(32) {
            world.write_message(KeyboardInput {
                key_code: key,
                logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        }
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert_eq!(world.resource::<PendingInput>().0.len(), 32);
        for turn in expected.into_iter().cycle().take(32) {
            world.run_system_once(read_keyboard).unwrap();
            assert_eq!(
                world.get::<ActionState<PlayerInput>>(player).unwrap().0,
                turn
            );
        }
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
    }

    #[test]
    fn rollback_and_unsynced_ticks_preserve_live_input() {
        let (mut world, client, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnLeft);
        world.entity_mut(client).insert(Rollback::FromState);
        // Single validation skips the writer, just like Lightyear's buffer.
        assert!(world.run_system_once(read_keyboard).is_err());
        assert_eq!(world.resource::<PendingInput>().0.len(), 1);
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        world.entity_mut(client).remove::<Rollback>();
        world.entity_mut(client).remove::<IsSynced<InputTimeline>>();
        assert!(world.run_system_once(read_keyboard).is_err());
        assert_eq!(world.resource::<PendingInput>().0.len(), 1);
        world
            .entity_mut(client)
            .insert(IsSynced::<InputTimeline>::default());
        world.run_system_once(read_keyboard).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnLeft
        );
    }

    #[test]
    fn missing_network_buffer_does_not_consume_turn() {
        let (mut world, _, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnRight);
        world
            .entity_mut(player)
            .remove::<NativeBuffer<PlayerInput>>();
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(world.resource::<PendingInput>().0.len(), 1);
        world
            .entity_mut(player)
            .insert(NativeBuffer::<PlayerInput>::default());
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnRight
        );
    }

    #[test]
    fn releases_repeats_and_unbound_keys_do_not_queue_turns() {
        let (mut world, _, _) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        for (key_code, state, repeat) in [
            (KeyCode::KeyA, ButtonState::Released, false),
            (KeyCode::KeyA, ButtonState::Pressed, true),
            (KeyCode::Space, ButtonState::Pressed, false),
        ] {
            world.write_message(KeyboardInput {
                key_code,
                logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                state,
                text: None,
                repeat,
                window: Entity::PLACEHOLDER,
            });
        }
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
    }

    #[test]
    fn death_respawn_and_disconnect_clear_only_live_queue() {
        let (mut world, client, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnLeft);
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        world.run_system_once(read_keyboard).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnRight);
        world
            .entity_mut(player)
            .insert((IsAlive(true), LifeGeneration(2)));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnRight);
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnRightFor(2)
        );
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnLeft);
        world.entity_mut(client).remove::<Connected>();
        world.init_resource::<Messages<KeyboardInput>>();
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
    }

    #[test]
    fn dead_presses_do_not_survive_into_new_life() {
        let (mut world, _, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        for key_code in [KeyCode::KeyA, KeyCode::KeyD] {
            world.write_message(KeyboardInput {
                key_code,
                logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        }
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        world
            .entity_mut(player)
            .insert((IsAlive(true), LifeGeneration(1)));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
    }

    #[test]
    fn respawn_key_requests_only_when_dead_and_never_resets_local_rider() {
        for key_code in [KeyCode::Space, KeyCode::Enter, KeyCode::NumpadEnter] {
            let (mut world, client, player) = setup();
            world.init_resource::<Messages<KeyboardInput>>();
            world
                .entity_mut(client)
                .insert(MessageSender::<RespawnRequest>::default());
            world.entity_mut(player).insert(LifeGeneration(4));
            let press = KeyboardInput {
                key_code,
                logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            };
            world.write_message(press.clone());
            world.run_system_once(buffer_keyboard_input).unwrap();
            assert!(world.resource::<RespawnUi>().pending_generation.is_none());
            world.get_mut::<IsAlive>(player).unwrap().0 = false;
            world.write_message(press.clone());
            world.run_system_once(buffer_keyboard_input).unwrap();
            assert_eq!(world.resource::<RespawnUi>().pending_generation, Some(4));
            assert!(!world.get::<IsAlive>(player).unwrap().0);
            assert_eq!(world.get::<LifeGeneration>(player).unwrap().0, 4);
            world.write_message(press);
            world.run_system_once(buffer_keyboard_input).unwrap();
            assert_eq!(world.resource::<RespawnUi>().pending_generation, Some(4));
            world
                .entity_mut(player)
                .insert((IsAlive(true), LifeGeneration(5)));
            world.run_system_once(buffer_keyboard_input).unwrap();
            assert!(world.resource::<RespawnUi>().pending_generation.is_none());
        }
    }

    #[test]
    fn keyboard_collection_during_rollback_keeps_fresh_turn_order() {
        let (mut world, client, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnRight);
        world.entity_mut(client).insert(Rollback::FromState);
        // Rewound dead state must not clear or replace the fresh live queue.
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        world.write_message(KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert_eq!(
            world.resource::<PendingInput>().0,
            VecDeque::from([PlayerInput::TurnRight, PlayerInput::TurnLeft])
        );
        assert!(world.run_system_once(read_keyboard).is_err());
        assert_eq!(world.resource::<PendingInput>().0.len(), 2);
    }

    #[test]
    fn menu_consumes_physical_keys_and_neutralizes_fresh_ticks() {
        let (mut world, _, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world.resource_mut::<MenuState>().open = true;
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PlayerInput::TurnRight);
        world.get_mut::<ActionState<PlayerInput>>(player).unwrap().0 = PlayerInput::TurnLeft;
        world.write_message(KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        world.run_system_once(buffer_keyboard_input).unwrap();
        world.run_system_once(read_keyboard).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
    }

    #[test]
    fn menu_enter_and_space_never_request_respawn_even_on_closing_frame() {
        for open in [true, false] {
            let (mut world, client, player) = setup();
            world.init_resource::<Messages<KeyboardInput>>();
            world.insert_resource(MenuState {
                open,
                block_this_frame: true,
            });
            world
                .entity_mut(client)
                .insert(MessageSender::<RespawnRequest>::default());
            world
                .entity_mut(player)
                .insert((IsAlive(false), LifeGeneration(4)));
            for key_code in [KeyCode::Space, KeyCode::Enter, KeyCode::NumpadEnter] {
                world.write_message(KeyboardInput {
                    key_code,
                    logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                    state: ButtonState::Pressed,
                    text: None,
                    repeat: false,
                    window: Entity::PLACEHOLDER,
                });
            }
            world.run_system_once(buffer_keyboard_input).unwrap();
            assert!(world.resource::<RespawnUi>().pending_generation.is_none());
            assert!(!world.get::<IsAlive>(player).unwrap().0);
        }
    }
}
