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
use shared::{
    ActionState, Direction, DisplayName, IsAlive, LifeGeneration, Player, PlayerId, PlayerInput,
    Position,
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
        // Same chain as the real client, minus the menu systems.
        app.add_systems(
            PreUpdate,
            (
                tc::menu::begin_input_frame,
                tc::attach_local_input,
                tc::connection::monitor_connection,
            )
                .chain()
                .after(bevy::input::InputSystems)
                .after(MessageSystems::Receive)
                .after(ReplicationSystems::Receive)
                .before(tc::input::buffer_keyboard_input),
        );
        app.add_systems(Update, tc::connection::finish_disconnect);
        app.add_systems(Update, tc::connection::send_leave);
        app.add_systems(
            PostUpdate,
            tc::connection::unlink_session.after(LinkSystems::Send),
        );
        tc::add_input_and_prediction(&mut app);
        app.init_resource::<ClientTrace>();
        app.add_systems(FixedLast, record_client_tick);
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
        self.app.world().resource::<Session>().phase
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

    /// Replicated id of the local rider, once it exists.
    pub fn player_id(&mut self) -> Option<String> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&PlayerId, (With<Predicted>, With<Controlled>)>();
        query.iter(world).next().map(|id| id.0.clone())
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

    /// Cheat: write a native Lightyear input for the next fixed tick without
    /// sending a `TurnCommand` (what a modified client could do).
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

#[allow(clippy::type_complexity)]
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
