//! Keyboard → turn-input mapping.
//!
//! `buffer_keyboard_input` (PreUpdate) reads raw `KeyboardInput` messages directly
//! — each physical key press is one event — and pushes each key's turn into a
//! queue, spread over subsequent ticks. `read_keyboard` (FixedPreUpdate,
//! WriteClientInputs) always writes the current tick's input — from the queue
//! if non-empty, else `None` — following the lightyear continuous-input pattern
//! where ActionState is set every new tick. Rollback replays historical input
//! without consuming the live queue.
//!
//! ONE-SHOT TURN CONTRACT: the native `ActionState` path above drives
//! prediction (and rollback replay). The turn itself reaches the server as a
//! sequenced `TurnCommand`, created in the same place the press is popped into
//! `ActionState`: `read_keyboard`. It goes into the [`TurnOutbox`], which
//! `send_turn_batch` repeats in every outgoing packet until the server reports
//! the command decided (see `outbox.rs`). Its `tick` is exactly the tick the
//! prediction applies the turn at (`LocalTimeline` + input delay, the formula
//! Lightyear's own input buffering uses), on the timeline that runs ahead of
//! the server. The server executes the command on that tick when it arrives in
//! time (see `tron_zero_server::turns`), so a delivered turn matches the
//! prediction and causes no correction. Rollback never pops the queue or
//! creates a command.
//!
//! PREDICTED DEATH IS NOT A REASON TO DROP A PRESS. `IsAlive` is predicted, so
//! a rider can be dead locally only because the prediction disagrees with the
//! opponents' interpolated trails while the server still has it alive. Presses
//! therefore keep flowing (the server is the judge and answers `NotAlive` if
//! the rider really is dead), and the queue is cleared only when the rider
//! entity or its life generation changes, on the menu, and on disconnect. The
//! generation tag on every press and command keeps anything from a previous
//! life from ever firing in the next one.

use std::collections::VecDeque;

use crate::chat::ChatState;
use crate::menu::MenuState;
use crate::settings::{RebindState, TurnBindings};
use bevy::ecs::message::MessageReader;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use lightyear::prelude::input::native::{InputMarker, NativeBuffer};
use lightyear::prelude::{
    Client, Connected, ControlledBy, InputTimeline, IsSynced, LocalTimeline, MessageReceiver,
    MessageSender, Rollback,
};
use shared::ActionState;
use shared::PlayerInput;
use shared::protocol::{
    RespawnChannel, RespawnOutcome, RespawnReply, RespawnRequest, TurnAck, TurnCommand,
    TurnDirection, TurnOutcome,
};

use super::outbox::TurnOutbox;
use shared::{IsAlive, LifeGeneration};

/// One queued physical press, tagged with the life it was pressed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingPress {
    pub direction: TurnDirection,
    pub generation: u128,
}

#[derive(Resource, Default)]
pub struct PendingInput(pub VecDeque<PendingPress>);

/// Next `TurnCommand::seq`. Monotonic for the whole connection: queue clears
/// (menu, death) must not rewind it, or the server would see duplicates.
#[derive(Resource, Default)]
pub struct TurnSeq(pub u64);

/// Last observed server ack (diagnostics only; prediction never waits for it).
#[derive(Resource, Default)]
pub struct TurnAckState {
    pub last_seq: Option<u64>,
    pub last_outcome: Option<TurnOutcome>,
    /// Commands below this seq are known to be decided by the server.
    pub received_below: u64,
}

/// The local rider the queue was last filled for: its entity and life. A
/// change of either drops the queued presses; its alive flag deliberately does
/// not (see the module docs).
#[derive(Resource, Default)]
pub struct InputLifecycle {
    observed: Option<(Entity, Option<u128>)>,
}

impl InputLifecycle {
    fn observe(&mut self, state: Option<(Entity, Option<u128>)>, pending: &mut PendingInput) {
        if self.observed != state {
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
    mut bindings: ResMut<TurnBindings>,
    mut rebind: ResMut<RebindState>,
    menu: Res<MenuState>,
    chat: Res<ChatState>,
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
            player.map(|(entity, _, life, _)| (entity, life.map(|l| l.0))),
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
    // Rebind capture eats every physical press; nothing reaches the turn queue.
    if let Some((side, index)) = rebind.capturing {
        for event in key_events.read() {
            if !event.state.is_pressed() || event.repeat {
                continue;
            }
            if event.key_code == KeyCode::Escape {
                rebind.capturing = None;
                rebind.error = None;
            } else {
                match bindings.apply_capture(side, index, event.key_code, &event.logical_key) {
                    Ok(()) => {
                        rebind.capturing = None;
                        rebind.error = None;
                    }
                    Err(message) => {
                        rebind.error = Some(message.to_owned());
                    }
                }
            }
            break;
        }
        pending.0.clear();
        key_events.clear();
        return;
    }
    if menu.captures_input() || chat.captures_input() {
        pending.0.clear();
        key_events.clear();
        return;
    }
    for event in key_events.read() {
        if !event.state.is_pressed() || event.repeat {
            continue;
        }
        let Some((_, alive, life, _)) = player else {
            continue;
        };
        let key = event.key_code;
        let direction = if bindings.is_left(key) {
            Some(TurnDirection::Left)
        } else if bindings.is_right(key) {
            Some(TurnDirection::Right)
        } else {
            None
        };
        if let Some(direction) = direction {
            // A rider without a life tag cannot turn authoritatively.
            if let Some(life) = life {
                pending.0.push_back(PendingPress {
                    direction,
                    generation: life.0,
                });
            }
        } else if key == KeyCode::Space
            && !rollback
            && !alive.0
            && respawn.pending_generation.is_none()
            && let (Some((client_entity, _)), Some(life)) = (client, life)
            && let Ok(mut sender) = senders.get_mut(client_entity)
        {
            sender.send::<RespawnChannel>(RespawnRequest { generation: life.0 });
            respawn.pending_generation = Some(life.0);
            respawn.outcome = None;
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

/// Stop repeating the commands the server has decided and record its newest
/// verdict. Prediction and rollback never wait for acks.
pub fn receive_turn_acks(
    mut clients: Query<&mut MessageReceiver<TurnAck>, (With<Client>, With<Connected>)>,
    mut acks: ResMut<TurnAckState>,
    mut outbox: ResMut<TurnOutbox>,
) {
    for mut receiver in &mut clients {
        for ack in receiver.receive() {
            outbox.acknowledge(ack.received_below);
            apply_turn_ack(&mut acks, ack);
        }
    }
}

/// Pure ack bookkeeping, split out so unit tests don't need Lightyear's
/// `pub(crate)` receiver internals.
fn apply_turn_ack(acks: &mut TurnAckState, ack: TurnAck) {
    // Acks are sequenced, but a stale one must never move the watermark back.
    acks.received_below = acks.received_below.max(ack.received_below);
    let Some(verdict) = ack.newest else {
        return;
    };
    acks.last_seq = Some(verdict.seq);
    acks.last_outcome = Some(verdict.outcome);
    match verdict.outcome {
        TurnOutcome::Applied { server_tick } => {
            debug!(seq = verdict.seq, server_tick, "Server applied turn");
        }
        TurnOutcome::Rejected { reason } => {
            debug!(seq = verdict.seq, ?reason, "Server rejected turn");
        }
    }
}

/// Pop at most one queued press into this tick's `ActionState` (prediction)
/// and queue the matching `TurnCommand`, stamped with this very tick, for
/// delivery.
///
/// Skipped during rollback (the `Without<Rollback>` filter), so replays reuse
/// Lightyear's recorded input history and never send or consume anything.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
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
    timeline: Option<Res<LocalTimeline>>,
    mut pending: ResMut<PendingInput>,
    mut lifecycle: ResMut<InputLifecycle>,
    mut turn_seq: ResMut<TurnSeq>,
    mut outbox: ResMut<TurnOutbox>,
    menu: Res<MenuState>,
    chat: Res<ChatState>,
    mut players: Query<
        (
            Entity,
            &mut ActionState<PlayerInput>,
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
    let (client_entity, input_timeline) = client.into_inner();
    let Some((entity, mut action, life, _)) =
        players.iter_mut().find(|(_, _, _, controlled_by)| {
            controlled_by.is_none_or(|controlled_by| controlled_by.owner == client_entity)
        })
    else {
        return;
    };
    lifecycle.observe(Some((entity, life.map(|l| l.0))), &mut pending);
    if menu.captures_input() || chat.captures_input() {
        pending.0.clear();
    }
    // Always set the value for this tick so lightyear can buffer it and
    // apply_turn reads the correct input. None means "no turn this tick",
    // matching lightyear's continuous-input contract. A locally dead rider
    // still pops: the death may be a misprediction (see the module docs).
    let press = pending
        .0
        .pop_front()
        // A press tagged for a prior life must never leak into the new life,
        // even if the lifecycle clear somehow missed it.
        .filter(|press| Some(press.generation) == life.map(|l| l.0));
    let Some(press) = press else {
        action.0 = PlayerInput::None;
        return;
    };
    action.0 = press.direction.to_input(press.generation);
    let seq = turn_seq.0;
    turn_seq.0 += 1;
    // Same tick (and delay) Lightyear's input buffering stamps this action with.
    let tick = timeline
        .map(|t| (t.tick() + input_timeline.input_delay() as i32).0)
        .unwrap_or(0);
    outbox.push(TurnCommand {
        seq,
        direction: press.direction,
        generation: press.generation,
        tick,
    });
    debug!(seq, ?press.direction, tick, "Turn command queued");
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::{ButtonState, keyboard::Key};
    use shared::protocol::TurnVerdict;

    fn setup() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<PendingInput>();
        world.init_resource::<InputLifecycle>();
        world.init_resource::<TurnSeq>();
        world.init_resource::<TurnAckState>();
        world.init_resource::<TurnOutbox>();
        world.init_resource::<RespawnUi>();
        world.init_resource::<ChatState>();
        world.insert_resource(MenuState {
            open: false,
            block_this_frame: false,
            screen: crate::menu::MenuScreen::Main,
        });
        world.init_resource::<TurnBindings>();
        world.init_resource::<RebindState>();
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
                LifeGeneration(1),
                InputMarker::<PlayerInput>::default(),
                NativeBuffer::<PlayerInput>::default(),
            ))
            .id();
        world.resource_mut::<InputLifecycle>().observed = Some((player, Some(1)));
        (world, client, player)
    }

    fn queued(input: PlayerInput) -> PendingPress {
        queued_for(input, 1)
    }

    fn queued_for(input: PlayerInput, generation: u128) -> PendingPress {
        let direction = match input {
            PlayerInput::TurnLeft => TurnDirection::Left,
            _ => TurnDirection::Right,
        };
        PendingPress {
            direction,
            generation,
        }
    }

    #[test]
    fn same_frame_presses_preserve_order_without_truncation() {
        let (mut world, _, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        let keys = [
            KeyCode::ArrowLeft,
            KeyCode::KeyQ,
            KeyCode::ArrowRight,
            KeyCode::KeyK,
        ];
        let expected = [
            PlayerInput::TurnLeftFor(1),
            PlayerInput::TurnLeftFor(1),
            PlayerInput::TurnRightFor(1),
            PlayerInput::TurnRightFor(1),
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
            .push_back(queued(PlayerInput::TurnLeft));
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
            PlayerInput::TurnLeftFor(1)
        );
    }

    #[test]
    fn missing_network_buffer_does_not_consume_turn() {
        let (mut world, _, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnRight));
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
            PlayerInput::TurnRightFor(1)
        );
    }

    #[test]
    fn releases_repeats_and_unbound_keys_do_not_queue_turns() {
        let (mut world, _, _) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        for (key_code, state, repeat) in [
            (KeyCode::KeyQ, ButtonState::Released, false),
            (KeyCode::KeyQ, ButtonState::Pressed, true),
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
    fn predicted_death_does_not_swallow_presses_but_a_new_life_drops_them() {
        let (mut world, client, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnLeft));
        // The prediction says dead (it may be wrong): the press still flows.
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        world.run_system_once(read_keyboard).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnLeftFor(1)
        );
        assert_eq!(world.resource::<TurnOutbox>().len(), 1);
        // A press from the old life is dropped when the life changes.
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnRight));
        world
            .entity_mut(player)
            .insert((IsAlive(true), LifeGeneration(2)));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        assert_eq!(world.resource::<TurnOutbox>().len(), 1);
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued_for(PlayerInput::TurnRight, 2));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnRightFor(2)
        );
        // Disconnect clears the live queue.
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnLeft));
        world.entity_mut(client).remove::<Connected>();
        world.init_resource::<Messages<KeyboardInput>>();
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
    }

    #[test]
    fn presses_while_locally_dead_fire_after_a_misprediction_but_not_in_a_new_life() {
        let (mut world, _, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        for key_code in [KeyCode::KeyQ, KeyCode::KeyK] {
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
        assert_eq!(
            world.resource::<PendingInput>().0.len(),
            2,
            "a locally dead rider's presses are kept"
        );
        // The server's state wins: the rider never died, same life.
        world.get_mut::<IsAlive>(player).unwrap().0 = true;
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnLeftFor(1)
        );
        // Whereas a genuine death followed by a respawn is a new life.
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        world
            .entity_mut(player)
            .insert((IsAlive(true), LifeGeneration(2)));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        assert!(world.resource::<PendingInput>().0.is_empty());
    }

    #[test]
    fn respawn_key_requests_only_when_dead_and_never_resets_local_rider() {
        {
            let key_code = KeyCode::Space;
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
            .push_back(queued(PlayerInput::TurnRight));
        world.entity_mut(client).insert(Rollback::FromState);
        // Rewound dead state must not clear or replace the fresh live queue.
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        world.write_message(KeyboardInput {
            key_code: KeyCode::KeyQ,
            logical_key: Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert_eq!(
            world
                .resource::<PendingInput>()
                .0
                .iter()
                .map(|p| p.direction)
                .collect::<Vec<_>>(),
            vec![TurnDirection::Right, TurnDirection::Left]
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
            .push_back(queued(PlayerInput::TurnRight));
        world.get_mut::<ActionState<PlayerInput>>(player).unwrap().0 = PlayerInput::TurnLeft;
        world.write_message(KeyboardInput {
            key_code: KeyCode::KeyQ,
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
    fn enter_is_the_chat_key_and_never_requests_respawn() {
        let (mut world, client, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world
            .entity_mut(client)
            .insert(MessageSender::<RespawnRequest>::default());
        world
            .entity_mut(player)
            .insert((IsAlive(false), LifeGeneration(4)));
        for key_code in [KeyCode::Enter, KeyCode::NumpadEnter] {
            press(&mut world, key_code);
        }
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<RespawnUi>().pending_generation.is_none());
    }

    #[test]
    fn typing_in_chat_swallows_turn_and_respawn_keys() {
        let (mut world, client, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world
            .entity_mut(client)
            .insert(MessageSender::<RespawnRequest>::default());
        world
            .entity_mut(player)
            .insert((IsAlive(false), LifeGeneration(4)));
        world.resource_mut::<ChatState>().typing = true;
        press(&mut world, KeyCode::Space);
        press(&mut world, KeyCode::ArrowLeft);
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<RespawnUi>().pending_generation.is_none());
        assert!(world.resource::<PendingInput>().0.is_empty());
    }

    #[test]
    fn menu_enter_and_space_never_request_respawn_even_on_closing_frame() {
        for open in [true, false] {
            let (mut world, client, player) = setup();
            world.init_resource::<Messages<KeyboardInput>>();
            world.insert_resource(MenuState {
                open,
                block_this_frame: true,
                screen: crate::menu::MenuScreen::Main,
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

    fn press(world: &mut World, key: KeyCode) {
        press_logical(
            world,
            key,
            Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
        );
    }

    fn press_logical(world: &mut World, key: KeyCode, logical_key: Key) {
        world.write_message(KeyboardInput {
            key_code: key,
            logical_key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    #[test]
    fn capture_drains_presses_without_queueing_turns() {
        use crate::settings::TurnSide;

        let (mut world, _, _) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world.resource_mut::<RebindState>().capturing = Some((TurnSide::Left, 0));
        // Duplicate of a right-side key: rejected, capture stays open.
        press(&mut world, KeyCode::KeyK);
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(
            world.resource::<RebindState>().capturing,
            Some((TurnSide::Left, 0))
        );
        assert!(world.resource::<RebindState>().error.is_some());
        // Every run_system_once gets a fresh reader; drop the consumed press.
        world.resource_mut::<Messages<KeyboardInput>>().clear();
        // Fresh unbound key: rebinds slot 0 and closes the capture.
        press(&mut world, KeyCode::KeyT);
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert!(world.resource::<RebindState>().capturing.is_none());
        assert!(world.resource::<RebindState>().error.is_none());
        assert_eq!(world.resource::<TurnBindings>().left[0].code, KeyCode::KeyT);
        world.resource_mut::<Messages<KeyboardInput>>().clear();
        // AZERTY glyph: physical KeyQ captured with logical "a" shows "A"
        // but still matches the physical code.
        world.resource_mut::<RebindState>().capturing = Some((TurnSide::Left, 0));
        press_logical(&mut world, KeyCode::KeyQ, Key::Character("a".into()));
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<RebindState>().capturing.is_none());
        assert_eq!(world.resource::<TurnBindings>().left[0].code, KeyCode::KeyQ);
        assert_eq!(
            crate::settings::key_label(&world.resource::<TurnBindings>().left[0]),
            "A"
        );
    }

    #[test]
    fn capture_escape_cancels_without_binding() {
        use crate::settings::TurnSide;

        let (mut world, _, _) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world.resource_mut::<RebindState>().capturing = Some((TurnSide::Right, 1));
        press(&mut world, KeyCode::Escape);
        world.run_system_once(buffer_keyboard_input).unwrap();
        assert!(world.resource::<RebindState>().capturing.is_none());
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert!(
            !world
                .resource::<TurnBindings>()
                .right
                .iter()
                .any(|b| b.code == KeyCode::Escape)
        );
    }

    #[test]
    fn presses_pop_in_order_one_per_tick_and_seq_counts_pops() {
        let (mut world, client, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        let _ = client;
        world.entity_mut(player).insert(LifeGeneration(9));
        world.resource_mut::<InputLifecycle>().observed = Some((player, Some(9)));
        for key in [KeyCode::KeyQ, KeyCode::KeyQ, KeyCode::KeyK] {
            press(&mut world, key);
        }
        world.run_system_once(buffer_keyboard_input).unwrap();
        // Queueing alone sends nothing and assigns no seq: that happens on pop,
        // at the tick the prediction applies the turn.
        assert_eq!(world.resource::<PendingInput>().0.len(), 3);
        assert_eq!(world.resource::<TurnSeq>().0, 0);
        // Identical consecutive turns stay distinct: Left, Left, Right.
        for (i, expected) in [
            PlayerInput::TurnLeftFor(9),
            PlayerInput::TurnLeftFor(9),
            PlayerInput::TurnRightFor(9),
        ]
        .into_iter()
        .enumerate()
        {
            world.run_system_once(read_keyboard).unwrap();
            assert_eq!(
                world.get::<ActionState<PlayerInput>>(player).unwrap().0,
                expected
            );
            assert_eq!(world.resource::<TurnSeq>().0, i as u64 + 1);
        }
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        assert_eq!(world.resource::<TurnSeq>().0, 3);
        // Each pop became a command waiting in the outbox, in order (this
        // world has no timeline, so they are all stamped tick 0).
        let sent = world.resource_mut::<TurnOutbox>().batch(0).unwrap();
        assert_eq!(
            sent.commands.iter().map(|c| c.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(sent.commands.iter().all(|c| c.generation == 9));
    }

    #[test]
    fn menu_clear_drops_presses_but_never_rewinds_seq() {
        let (mut world, _, player) = setup();
        world.resource_mut::<TurnSeq>().0 = 5;
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnLeft));
        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(read_keyboard).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(world.resource::<TurnSeq>().0, 5);
        world.resource_mut::<MenuState>().open = false;
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnRight));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnRightFor(1)
        );
        assert_eq!(world.resource::<TurnSeq>().0, 6);
    }

    #[test]
    fn life_change_drops_stale_press_without_consuming_seq() {
        let (mut world, _, player) = setup();
        // A press tagged for the old life sits in the queue...
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued_for(PlayerInput::TurnLeft, 7));
        // ...then the rider respawns into a new life.
        world.entity_mut(player).insert(LifeGeneration(8));
        world.run_system_once(read_keyboard).unwrap();
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(world.resource::<TurnSeq>().0, 0);
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        // Even without the lifecycle clear, a stale press never leaks.
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued_for(PlayerInput::TurnRight, 7));
        world.resource_mut::<InputLifecycle>().observed = Some((player, Some(8)));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        assert_eq!(world.resource::<TurnSeq>().0, 0);
    }

    #[test]
    fn turn_acks_record_last_verdict_without_touching_prediction() {
        let (mut world, _, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnLeft));
        apply_turn_ack(
            &mut world.resource_mut::<TurnAckState>(),
            TurnAck {
                received_below: 5,
                newest: Some(TurnVerdict {
                    seq: 4,
                    outcome: TurnOutcome::Applied { server_tick: 900 },
                }),
            },
        );
        let acks = world.resource::<TurnAckState>();
        assert_eq!(acks.received_below, 5);
        assert_eq!(acks.last_seq, Some(4));
        assert_eq!(
            acks.last_outcome,
            Some(TurnOutcome::Applied { server_tick: 900 })
        );
        assert_eq!(world.resource::<PendingInput>().0.len(), 1);
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
    }
}
