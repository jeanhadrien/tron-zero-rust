//! Headless client for network tests: the production network, connection,
//! input and prediction systems (`tron_zero_client`), with scripted
//! `KeyboardInput` events instead of a window.

use std::collections::{BTreeMap, VecDeque};

use bevy::ecs::system::RunSystemOnce;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput, NativeKey};
use bevy::prelude::*;
use glam::Vec2;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;
use shared::protocol::{ChatChannel, ChatSend, TurnBatch, TurnChannel};
use shared::{
    ActionState, Direction, DisplayName, IsAlive, LifeGeneration, Player, PlayerColor, PlayerId,
    PlayerInput, Position, RiderShape,
};
use tc::connection::{ConnectionPhase, Session};
use tc::input::{PendingInput, TurnAckState};
use tc::menu::MenuState;
use tron_zero_client as tc;

use crate::{Impairment, install_conditioner};

/// Default bindings: both of these are bound by `TurnBindings::default()`.
pub const LEFT: KeyCode = KeyCode::ArrowLeft;
pub const RIGHT: KeyCode = KeyCode::ArrowRight;

/// The predicted rider at the end of one fixed tick (last execution wins, so
/// after a rollback this is the replayed value).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientTick {
    pub position: Vec2,
    pub direction: Vec2,
    pub executed: PlayerInput,
}

/// Prediction history of the local rider.
#[derive(Resource, Default)]
pub struct ClientTrace {
    pub history: BTreeMap<u32, ClientTick>,
    /// Input of each tick the first time it ran outside a rollback: the turns
    /// the player actually "pressed" on the predicted timeline.
    pub first_input: BTreeMap<u32, PlayerInput>,
    /// Ticks re-simulated by rollbacks, and those whose input differed from
    /// the original run (must stay empty).
    pub replayed_ticks: u32,
    pub replay_input_mismatches: Vec<u32>,
    /// Non-rollback executions of a tick that already ran (timeline jumps).
    pub reexecuted_ticks: Vec<u32>,
    /// Tick deltas of the timeline resyncs Lightyear applied.
    pub resyncs: Vec<i32>,
}

impl ClientTrace {
    /// Renumber everything recorded so far by `delta` ticks, as Lightyear does
    /// to its own prediction and input histories on a timeline resync.
    fn shift_ticks(&mut self, delta: i32) {
        fn shifted<T>(map: &mut BTreeMap<u32, T>, delta: i32) {
            *map = std::mem::take(map)
                .into_iter()
                .map(|(tick, value)| (tick.saturating_add_signed(delta), value))
                .collect();
        }
        shifted(&mut self.history, delta);
        shifted(&mut self.first_input, delta);
        self.resyncs.push(delta);
    }
}

/// A scripted physical press: injected once the client timeline reaches `tick`.
#[derive(Clone, Copy, Debug)]
struct ScriptedPress {
    tick: u32,
    key: KeyCode,
}

pub struct TestClient {
    pub app: App,
    script: VecDeque<ScriptedPress>,
    /// Client tick at each injection (what the old press-time stamp was).
    pub injected_at: Vec<u32>,
}

impl TestClient {
    pub fn new(impairment: Impairment) -> Self {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        tc::add_network(&mut app);
        app.init_resource::<Session>();
        app.init_resource::<MenuState>();
        app.init_resource::<tc::settings::TurnBindings>();
        app.init_resource::<tc::settings::RebindState>();
        // The real client's lifecycle wiring, minus the menu systems.
        tc::connection::add_lifecycle_systems(&mut app);
        app.add_systems(
            PreUpdate,
            tc::chat::chat_controls
                .after(tc::connection::monitor_connection)
                .before(tc::input::buffer_keyboard_input),
        );
        tc::add_input_and_prediction(&mut app);
        app.init_resource::<ClientTrace>();
        app.add_systems(FixedLast, record_client_tick);
        app.add_observer(follow_timeline_resync);
        install_conditioner(&mut app, impairment);
        app.finish();
        app.cleanup();
        Self {
            app,
            script: VecDeque::new(),
            injected_at: Vec::new(),
        }
    }

    /// Like [`TestClient::new`], plus the client-side network simulation the
    /// real client enables with `TRON_SIM_LATENCY_MS` (on top of `impairment`).
    pub fn with_sim(impairment: Impairment, sim: tc::sim::NetSim) -> Self {
        let mut client = Self::new(impairment);
        sim.install(&mut client.app);
        client
    }

    /// Start the production connect flow towards `127.0.0.1:port` as "Player".
    pub fn connect(&mut self, port: u16) {
        self.connect_as(port, "Player");
    }

    /// Start the production connect flow, asking for `name`.
    pub fn connect_as(&mut self, port: u16, name: &'static str) {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        self.app
            .world_mut()
            .run_system_once(
                move |mut session: ResMut<Session>,
                      time: Res<Time<Real>>,
                      mut commands: Commands| {
                    session.connect(address, name, time.elapsed_secs_f64(), &mut commands);
                },
            )
            .expect("connect system runs");
    }

    /// Like [`TestClient::connect_as`], but over WebTransport to
    /// `127.0.0.1:web_port`, trusting the server certificate with `cert_hash`
    /// (lowercase hex SHA-256, no colons: what a browser would be given).
    pub fn connect_web_as(&mut self, web_port: u16, cert_hash: &str, name: &'static str) {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], web_port));
        let certificate_digest = cert_hash.to_owned();
        self.app
            .world_mut()
            .run_system_once(
                move |mut session: ResMut<Session>,
                      time: Res<Time<Real>>,
                      mut commands: Commands| {
                    // `RawClient` becomes `Connected` when `Linked` is added and
                    // reads `LocalAddr` at that moment; the WebTransport link
                    // only learns its address afterwards, so seed a placeholder.
                    let io = (
                        client::WebTransportClientIo {
                            certificate_digest: certificate_digest.clone(),
                        },
                        LocalAddr(std::net::SocketAddr::from(([0, 0, 0, 0], 0))),
                        PeerAddr(address),
                    );
                    session.connect_with(address, io, name, time.elapsed_secs_f64(), &mut commands);
                },
            )
            .expect("connect system runs");
    }

    pub fn session(&self) -> &Session {
        self.app.world().resource::<Session>()
    }

    /// Protocol version the next hello will advertise (fake a stale build).
    pub fn set_protocol(&mut self, protocol: u32) {
        self.app.world_mut().resource_mut::<Session>().protocol = protocol;
    }

    /// Names of every rider this client knows (its own and replicated others).
    pub fn rider_names(&mut self) -> Vec<String> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&DisplayName, With<Player>>();
        let mut names: Vec<String> = query.iter(world).map(|name| name.0.clone()).collect();
        names.sort();
        names
    }

    /// Measured round-trip time of the client link, as Lightyear estimates it.
    pub fn link_rtt(&mut self) -> std::time::Duration {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&Link, With<Client>>();
        query
            .iter(world)
            .next()
            .map_or(std::time::Duration::ZERO, |link| link.stats.rtt)
    }

    /// Start the production disconnect flow.
    pub fn disconnect(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<Session>()
            .disconnect("test disconnect");
    }

    pub fn phase(&self) -> ConnectionPhase {
        self.app.world().resource::<Session>().phase()
    }

    pub fn is_playing(&self) -> bool {
        self.phase() == ConnectionPhase::Playing
    }

    /// The client's current simulation tick (the prediction timeline).
    pub fn tick(&self) -> u32 {
        self.app.world().resource::<LocalTimeline>().tick().0
    }

    pub fn trace(&self) -> &ClientTrace {
        self.app.world().resource::<ClientTrace>()
    }

    /// Turn commands the client has sent so far.
    pub fn turns_sent(&self) -> u64 {
        self.app.world().resource::<tc::input::TurnSeq>().0
    }

    /// Turn commands still being repeated because the server has not reported
    /// them decided.
    pub fn turns_unacked(&self) -> usize {
        self.app.world().resource::<tc::input::TurnOutbox>().len()
    }

    /// Stamps of the commands still being repeated, oldest first.
    pub fn outbox_stamps(&self) -> Vec<u32> {
        self.app
            .world()
            .resource::<tc::input::TurnOutbox>()
            .stamps()
            .collect()
    }

    /// Cheat: put a hand-made batch on the wire next to the real ones (what
    /// a modified client flooding the server would do).
    pub fn send_raw_turn_batch(&mut self, batch: TurnBatch) {
        let world = self.app.world_mut();
        let mut senders = world.query_filtered::<&mut MessageSender<TurnBatch>, With<Client>>();
        for mut sender in senders.iter_mut(world) {
            sender.send::<TurnChannel>(batch.clone());
        }
    }

    pub fn pending_presses(&self) -> usize {
        self.app.world().resource::<PendingInput>().0.len()
    }

    pub fn last_ack(&self) -> &TurnAckState {
        self.app.world().resource::<TurnAckState>()
    }

    pub fn rollbacks(&self) -> u32 {
        self.app
            .world()
            .get_resource::<PredictionMetrics>()
            .map_or(0, |m| m.rollbacks)
    }

    /// How this client classifies each rider it knows, sorted by name.
    pub fn rider_roles(&mut self) -> Vec<RiderRole> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<(
            &DisplayName,
            Has<Predicted>,
            Has<Controlled>,
            Has<Interpolated>,
        ), With<Player>>();
        let mut roles: Vec<RiderRole> = query
            .iter(world)
            .map(|(name, predicted, controlled, interpolated)| RiderRole {
                name: name.0.clone(),
                predicted,
                controlled,
                interpolated,
            })
            .collect();
        roles.sort_by(|a, b| a.name.cmp(&b.name));
        roles
    }

    /// Replicated id of the local rider, once it exists.
    pub fn player_id(&mut self) -> Option<String> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&PlayerId, (With<Predicted>, With<Controlled>)>();
        query.iter(world).next().map(|id| id.0.clone())
    }

    /// This client's current `Trail` corners for the rider `player_id`: the
    /// predicted value for its own rider, the interpolated one for others.
    pub fn trail_of(&mut self, player_id: &str) -> Option<shared::Trail> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<(&PlayerId, &shared::Trail), With<Player>>();
        query
            .iter(world)
            .find(|(id, _)| id.0 == player_id)
            .map(|(_, trail)| trail.clone())
    }

    pub fn rider_alive(&mut self) -> bool {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&IsAlive, (With<Predicted>, With<Controlled>)>();
        query.iter(world).next().is_some_and(|alive| alive.0)
    }

    pub fn rider_generation(&mut self) -> Option<u128> {
        let world = self.app.world_mut();
        let mut query =
            world.query_filtered::<&LifeGeneration, (With<Predicted>, With<Controlled>)>();
        query.iter(world).next().map(|life| life.0)
    }

    /// The match status this client has received, once replicated.
    pub fn match_status(&mut self) -> Option<shared::MatchStatus> {
        let world = self.app.world_mut();
        let mut query = world.query::<&shared::MatchStatus>();
        query.iter(world).next().cloned()
    }

    /// The scores this client sees, by rider name, sorted by name.
    pub fn scores(&mut self) -> Vec<(String, shared::Score)> {
        let world = self.app.world_mut();
        let mut query =
            world.query_filtered::<(&shared::DisplayName, &shared::Score), With<Player>>();
        let mut scores: Vec<_> = query
            .iter(world)
            .map(|(name, score)| (name.0.clone(), *score))
            .collect();
        scores.sort_by(|a, b| a.0.cmp(&b.0));
        scores
    }

    /// The server's last answer to this client's respawn request.
    pub fn respawn_outcome(&self) -> Option<shared::protocol::RespawnOutcome> {
        self.app.world().resource::<tc::input::RespawnUi>().outcome
    }

    /// Queue a press for when the client timeline reaches `tick`.
    pub fn schedule(&mut self, tick: u32, key: KeyCode) {
        self.script.push_back(ScriptedPress { tick, key });
    }

    pub fn scripted_remaining(&self) -> usize {
        self.script.len()
    }

    /// Inject one physical key press right now.
    pub fn press(&mut self, key: KeyCode) {
        self.injected_at.push(self.tick());
        self.app.world_mut().write_message(KeyboardInput {
            key_code: key,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// One physical key press of the chat box: `logical_key` plus the text it
    /// would type, if any.
    fn press_chat_key(&mut self, logical_key: Key, text: Option<&str>) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Unidentified(bevy::input::keyboard::NativeKeyCode::Unidentified),
            logical_key,
            state: ButtonState::Pressed,
            text: text.map(Into::into),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// Type `text` into the chat box like a player: Enter, the characters one
    /// key press at a time, Enter. Each press gets its own frame.
    pub fn say(&mut self, text: &str) {
        self.press_chat_key(Key::Enter, None);
        self.app.update();
        for c in text.chars() {
            let typed = c.to_string();
            self.press_chat_key(Key::Character(typed.as_str().into()), Some(&typed));
            self.app.update();
        }
        self.press_chat_key(Key::Enter, None);
        self.app.update();
    }

    /// Chat lines received so far as `name: text`, oldest first.
    pub fn chat_lines(&self) -> Vec<String> {
        self.app
            .world()
            .resource::<tc::chat::ChatLog>()
            .0
            .iter()
            .map(|entry| format!("{}: {}", entry.name, entry.text))
            .collect()
    }

    /// Sender color of each chat line received so far, oldest first.
    pub fn chat_colors(&self) -> Vec<u32> {
        self.app
            .world()
            .resource::<tc::chat::ChatLog>()
            .0
            .iter()
            .map(|entry| entry.color)
            .collect()
    }

    /// Choose the rider look, as the menu does. Before connecting it is sent
    /// with the hello; afterwards it is a live change.
    pub fn set_look(&mut self, color: u32, shape: RiderShape) {
        self.app
            .world_mut()
            .insert_resource(tc::look::Look { color, shape });
    }

    /// Color and shape of the rider named `name` as this client sees it
    /// (its own predicted rider or a replicated copy).
    pub fn look_of(&mut self, name: &str) -> Option<(u32, RiderShape)> {
        let world = self.app.world_mut();
        let mut query =
            world.query_filtered::<(&DisplayName, &PlayerColor, &RiderShape), With<Player>>();
        query
            .iter(world)
            .find(|(display, ..)| display.0 == name)
            .map(|(_, color, shape)| (color.0, *shape))
    }

    /// Color and shape on this client's own predicted, controlled rider (the
    /// one the renderer draws for the local player).
    pub fn own_look(&mut self) -> Option<(u32, RiderShape)> {
        let world = self.app.world_mut();
        let mut query = world
            .query_filtered::<(&PlayerColor, &RiderShape), (With<Predicted>, With<Controlled>)>();
        query
            .iter(world)
            .next()
            .map(|(color, shape)| (color.0, *shape))
    }

    /// Cheat: put a hand-made chat message on the wire (what a modified client
    /// would send).
    pub fn send_raw_chat(&mut self, text: &str) {
        let world = self.app.world_mut();
        let mut senders = world.query_filtered::<&mut MessageSender<ChatSend>, With<Client>>();
        for mut sender in senders.iter_mut(world) {
            sender.send::<ChatChannel>(ChatSend { text: text.into() });
        }
    }

    /// Open or close the in-game menu (which swallows turn keys).
    pub fn set_menu_open(&mut self, open: bool) {
        self.app.world_mut().resource_mut::<MenuState>().open = open;
    }

    /// Force a rollback to `tick` on the next frame.
    pub fn force_rollback(&mut self, tick: u32) {
        self.app
            .world_mut()
            .resource_mut::<StateRollbackMetadata>()
            .request_forced_rollback(Tick(tick));
    }

    /// Relabel the client timeline by `delta` ticks, exactly as a Lightyear
    /// resync does (the same `SyncEvent` the sync system triggers). Lightyear's
    /// own histories and the turn outbox follow; nothing is re-simulated. The
    /// presses still scripted are renumbered too: the player does not press
    /// at a different moment because the tick labels moved.
    pub fn force_resync(&mut self, delta: i32) {
        for press in &mut self.script {
            press.tick = press.tick.saturating_add_signed(delta);
        }
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<Entity, With<Client>>();
        let client = query.single(world).expect("one client entity");
        world.trigger(SyncEvent::<client::InputTimelineConfig>::new(client, delta));
    }

    /// Cheat: write a native Lightyear input for the next fixed tick without
    /// sending a turn command (what a modified client could do).
    pub fn plant_native_input(&mut self, input: PlayerInput) {
        self.app
            .world_mut()
            .insert_resource(PlantedInput(Some(input)));
        if !self.app.world().contains_resource::<PlantInstalled>() {
            self.app.world_mut().insert_resource(PlantInstalled);
            self.app.add_systems(
                FixedPreUpdate,
                plant_input
                    .after(tc::input::read_keyboard)
                    .before(lightyear::prelude::client::input::InputSystems::BufferClientInputs),
            );
        }
    }

    /// Inject due scripted presses, then run one frame.
    pub fn update(&mut self) {
        let tick = self.tick();
        while self.script.front().is_some_and(|press| press.tick <= tick) {
            let press = self.script.pop_front().expect("front exists");
            self.press(press.key);
        }
        self.app.update();
    }
}

#[derive(Resource)]
struct PlantInstalled;

#[derive(Resource)]
struct PlantedInput(Option<PlayerInput>);

/// Overwrite the local rider's `ActionState` once, after the real writer.
fn plant_input(
    mut planted: ResMut<PlantedInput>,
    mut riders: Query<&mut ActionState<PlayerInput>, With<InputMarker<PlayerInput>>>,
) {
    if let Some(input) = planted.0.take() {
        for mut action in &mut riders {
            action.0 = input;
        }
    }
}

/// A `SyncEvent` moves the client's tick counter (initially, and again when the
/// sync error exceeds `max_error_margin`, e.g. after a lost-ping streak). No
/// simulation step is re-run or rewound: the rider just continues, but its
/// next ticks carry numbers already used. Lightyear relabels its histories with
/// `update_ticks(delta)` (`lightyear_prediction::predicted_history`), so the
/// trace must follow or those fresh ticks look like double executions.
fn follow_timeline_resync(
    event: On<SyncEvent<client::InputTimelineConfig>>,
    mut trace: ResMut<ClientTrace>,
) {
    trace.shift_ticks(event.tick_delta);
}

fn record_client_tick(
    timeline: Res<LocalTimeline>,
    rollback: Query<(), (With<Client>, With<Rollback>)>,
    riders: Query<
        (&Position, &Direction, &ActionState<PlayerInput>),
        (With<Predicted>, With<Controlled>),
    >,
    mut trace: ResMut<ClientTrace>,
) {
    let tick = timeline.tick().0;
    let in_rollback = !rollback.is_empty();
    for (position, direction, action) in &riders {
        trace.history.insert(
            tick,
            ClientTick {
                position: position.0,
                direction: direction.0,
                executed: action.0,
            },
        );
        if in_rollback {
            trace.replayed_ticks += 1;
            let original = trace.first_input.get(&tick).copied();
            if original.is_some_and(|input| input != action.0) {
                trace.replay_input_mismatches.push(tick);
            }
        } else if trace.first_input.insert(tick, action.0).is_some() {
            trace.reexecuted_ticks.push(tick);
        }
    }
}

/// `(tick, input)` of every turn in a first-run input map, in tick order.
pub fn turns_of(inputs: &BTreeMap<u32, PlayerInput>) -> Vec<(u32, PlayerInput)> {
    inputs
        .iter()
        .filter(|(_, input)| **input != PlayerInput::None)
        .map(|(tick, input)| (*tick, *input))
        .collect()
}

/// Which of Lightyear's per-client markers a replicated rider carries on a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiderRole {
    pub name: String,
    pub predicted: bool,
    pub controlled: bool,
    pub interpolated: bool,
}
