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
//! ONE-SHOT TURN CONTRACT (phantom-turn fix): the native `ActionState` path above
//! is prediction only. In parallel, `buffer_keyboard_input` sends each physical
//! press as a sequenced `TurnCommand` over the ordered-reliable `TurnChannel`.
//! The server executes each `seq` at most once, in order, on the next available
//! fixed tick(s) after receipt (bounded late execution — never strict
//! intended-tick rejection, which is unworkable with zero input lead), and
//! overwrites `ActionState` from its queue every tick, so Lightyear's
//! `get_predict` repeat-last fallback is irrelevant on the server. Missing
//! input executes nothing. Send order equals prediction pop order (single FIFO).
//! Rollback story: the server never rolls back so its queue needs no rollback
//! handling; client replay uses the existing native `InputBuffer` history, fed
//! the same ordered press sequence, while duplicate packets never double-fire
//! (server dedups by `(generation, seq)`).

use std::collections::VecDeque;

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
    RespawnChannel, RespawnOutcome, RespawnReply, RespawnRequest, TurnAck, TurnChannel,
    TurnCommand, TurnDirection, TurnOutcome,
};
use shared::{IsAlive, LifeGeneration};

/// One queued physical press: the prediction input plus the identity of the
/// parallel `TurnCommand` sent for it. `seq` is `None` only when the rider has
/// no life to tag — prediction only, no command sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingPress {
    pub input: PlayerInput,
    pub seq: Option<u64>,
    pub generation: Option<u128>,
    pub client_tick: u32,
}

impl PendingPress {
    fn prediction_only(input: PlayerInput) -> Self {
        Self {
            input,
            seq: None,
            generation: None,
            client_tick: 0,
        }
    }
}

#[derive(Resource, Default)]
pub struct PendingInput(pub VecDeque<PendingPress>);

/// Per-link monotonic turn sequence. Reset exactly when `InputLifecycle`
/// clears the queue (death / life-change / disconnect / menu-capture), so send
/// order always matches prediction pop order from a clean slate.
#[derive(Resource, Default)]
pub struct TurnSeq(pub u64);

/// Last observed server ack (diagnostics only; prediction never waits for it).
#[derive(Resource, Default)]
pub struct TurnAckState {
    pub last_seq: Option<u64>,
    pub last_outcome: Option<TurnOutcome>,
}

#[derive(Resource, Default)]
pub struct InputLifecycle {
    observed: Option<(Entity, bool, Option<u128>)>,
}

impl InputLifecycle {
    fn observe(
        &mut self,
        state: Option<(Entity, bool, Option<u128>)>,
        pending: &mut PendingInput,
        turn_seq: &mut TurnSeq,
    ) {
        if self.observed != state || state.is_none_or(|(_, alive, _)| !alive) {
            pending.0.clear();
            turn_seq.0 = 0;
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
    mut turn_seq: ResMut<TurnSeq>,
    mut respawn: ResMut<RespawnUi>,
    mut bindings: ResMut<TurnBindings>,
    mut rebind: ResMut<RebindState>,
    menu: Res<MenuState>,
    timeline: Option<Res<LocalTimeline>>,
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
    mut turn_senders: Query<&mut MessageSender<TurnCommand>>,
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
            &mut turn_seq,
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
        turn_seq.0 = 0;
        key_events.clear();
        return;
    }
    if menu.captures_input() {
        pending.0.clear();
        turn_seq.0 = 0;
        key_events.clear();
        return;
    }
    let can_turn = lifecycle.observed.is_some_and(|(_, alive, _)| alive);
    let client_tick = timeline.map(|t| t.tick().0).unwrap_or(0);
    for event in key_events.read() {
        if !event.state.is_pressed() || event.repeat {
            continue;
        }
        let Some((_, alive, life, _)) = player else {
            continue;
        };
        let key = event.key_code;
        if bindings.is_left(key) && can_turn {
            push_turn(
                &mut pending,
                &mut turn_seq,
                life,
                client_tick,
                TurnDirection::Left,
                client,
                &mut turn_senders,
            );
        } else if bindings.is_right(key) && can_turn {
            push_turn(
                &mut pending,
                &mut turn_seq,
                life,
                client_tick,
                TurnDirection::Right,
                client,
                &mut turn_senders,
            );
        } else if matches!(key, KeyCode::Space | KeyCode::Enter | KeyCode::NumpadEnter)
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

/// Queue one physical press for prediction and, in parallel, send the matching
/// sequenced `TurnCommand`. Single FIFO: the `seq` assignment order is the pop
/// order `read_keyboard` will use, so prediction and authority agree.
#[allow(clippy::too_many_arguments)]
fn push_turn(
    pending: &mut PendingInput,
    turn_seq: &mut TurnSeq,
    life: Option<&LifeGeneration>,
    client_tick: u32,
    direction: TurnDirection,
    client: Option<(Entity, bool)>,
    turn_senders: &mut Query<&mut MessageSender<TurnCommand>>,
) {
    let input = match direction {
        TurnDirection::Left => PlayerInput::TurnLeft,
        TurnDirection::Right => PlayerInput::TurnRight,
    };
    let generation = life.map(|l| l.0);
    let Some(generation) = generation else {
        // No life to tag: prediction only, no authoritative command.
        pending.0.push_back(PendingPress::prediction_only(input));
        return;
    };
    let seq = turn_seq.0;
    turn_seq.0 = turn_seq.0.wrapping_add(1);
    pending.0.push_back(PendingPress {
        input,
        seq: Some(seq),
        generation: Some(generation),
        client_tick,
    });
    if let Some((client_entity, _)) = client
        && let Ok(mut sender) = turn_senders.get_mut(client_entity)
    {
        sender.send::<TurnChannel>(TurnCommand {
            seq,
            direction,
            generation,
            client_tick,
        });
        debug!(
            seq,
            ?direction,
            generation,
            client_tick,
            "Turn command sent"
        );
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

/// Record the server's per-command verdict. Diagnostics only: prediction and
/// rollback never wait for acks.
pub fn receive_turn_acks(
    mut clients: Query<&mut MessageReceiver<TurnAck>, (With<Client>, With<Connected>)>,
    mut acks: ResMut<TurnAckState>,
) {
    for mut receiver in &mut clients {
        for ack in receiver.receive() {
            apply_turn_ack(&mut acks, ack);
        }
    }
}

/// Pure ack bookkeeping, split out so unit tests don't need Lightyear's
/// `pub(crate)` receiver internals.
fn apply_turn_ack(acks: &mut TurnAckState, ack: TurnAck) {
    acks.last_seq = Some(ack.seq);
    acks.last_outcome = Some(ack.outcome);
    match ack.outcome {
        TurnOutcome::Applied { server_tick } => {
            debug!(seq = ack.seq, server_tick, "Server applied turn");
        }
        TurnOutcome::Rejected { reason } => {
            debug!(seq = ack.seq, ?reason, "Server rejected turn");
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
    mut turn_seq: ResMut<TurnSeq>,
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
    lifecycle.observe(
        Some((entity, alive.0, life.map(|l| l.0))),
        &mut pending,
        &mut turn_seq,
    );
    if menu.captures_input() {
        pending.0.clear();
        turn_seq.0 = 0;
    }
    // Always set the value for this tick so lightyear can buffer it and
    // apply_turn reads the correct input. None means "no turn this tick",
    // matching lightyear's continuous-input contract.
    let input = if alive.0 {
        match pending.0.pop_front() {
            Some(press) => {
                // A press tagged for a prior life must never leak into the new
                // life, even if the lifecycle clear somehow missed it.
                if press
                    .generation
                    .is_some_and(|g| Some(g) != life.map(|l| l.0))
                {
                    PlayerInput::None
                } else {
                    press.input
                }
            }
            None => PlayerInput::None,
        }
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
        world.init_resource::<TurnSeq>();
        world.init_resource::<TurnAckState>();
        world.init_resource::<RespawnUi>();
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
                InputMarker::<PlayerInput>::default(),
                NativeBuffer::<PlayerInput>::default(),
            ))
            .id();
        world.resource_mut::<InputLifecycle>().observed = Some((player, true, None));
        (world, client, player)
    }

    fn queued(input: PlayerInput) -> PendingPress {
        PendingPress::prediction_only(input)
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
            PlayerInput::TurnLeft
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
            PlayerInput::TurnRight
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
    fn death_respawn_and_disconnect_clear_only_live_queue() {
        let (mut world, client, player) = setup();
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(queued(PlayerInput::TurnLeft));
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
            .push_back(queued(PlayerInput::TurnRight));
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
            .push_back(queued(PlayerInput::TurnRight));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::TurnRightFor(2)
        );
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
    fn dead_presses_do_not_survive_into_new_life() {
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
                .map(|p| p.input)
                .collect::<Vec<_>>(),
            vec![PlayerInput::TurnRight, PlayerInput::TurnLeft]
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
    fn turn_presses_assign_link_seq_in_send_order() {
        let (mut world, client, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world
            .entity_mut(client)
            .insert(MessageSender::<TurnCommand>::default());
        world.entity_mut(player).insert(LifeGeneration(9));
        for key in [KeyCode::KeyQ, KeyCode::KeyQ, KeyCode::KeyK] {
            press(&mut world, key);
        }
        world.run_system_once(buffer_keyboard_input).unwrap();
        let queued: Vec<_> = world.resource::<PendingInput>().0.iter().collect();
        assert_eq!(queued.len(), 3);
        // Identical consecutive turns keep their own seq: Left, Left, Right.
        assert_eq!(
            queued.iter().map(|p| p.input).collect::<Vec<_>>(),
            vec![
                PlayerInput::TurnLeft,
                PlayerInput::TurnLeft,
                PlayerInput::TurnRight
            ]
        );
        assert_eq!(
            queued.iter().map(|p| p.seq).collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(2)]
        );
        assert!(queued.iter().all(|p| p.generation == Some(9)));
        assert_eq!(world.resource::<TurnSeq>().0, 3);
        // Prediction pops in the same order the commands were sent.
        for expected in [PlayerInput::TurnLeftFor(9), PlayerInput::TurnLeftFor(9)] {
            world.run_system_once(read_keyboard).unwrap();
            assert_eq!(
                world.get::<ActionState<PlayerInput>>(player).unwrap().0,
                expected
            );
        }
    }

    #[test]
    fn life_change_resets_seq_and_drops_stale_press() {
        let (mut world, _, player) = setup();
        world.init_resource::<Messages<KeyboardInput>>();
        world.entity_mut(player).insert(LifeGeneration(7));
        world.resource_mut::<TurnSeq>().0 = 2;
        // A press tagged for the old life sits in the queue...
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PendingPress {
                input: PlayerInput::TurnLeft,
                seq: Some(1),
                generation: Some(7),
                client_tick: 0,
            });
        // ...then the rider respawns into a new life.
        world.entity_mut(player).insert(LifeGeneration(8));
        world.run_system_once(read_keyboard).unwrap();
        // Lifecycle clear drops the queue, resets seq, and writes nothing.
        assert!(world.resource::<PendingInput>().0.is_empty());
        assert_eq!(world.resource::<TurnSeq>().0, 0);
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        // Even without the lifecycle clear, a stale press never leaks tags.
        world
            .resource_mut::<PendingInput>()
            .0
            .push_back(PendingPress {
                input: PlayerInput::TurnRight,
                seq: Some(0),
                generation: Some(7),
                client_tick: 0,
            });
        world.resource_mut::<InputLifecycle>().observed = Some((player, true, Some(8)));
        world.run_system_once(read_keyboard).unwrap();
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
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
                seq: 4,
                outcome: TurnOutcome::Applied { server_tick: 900 },
            },
        );
        let acks = world.resource::<TurnAckState>();
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
