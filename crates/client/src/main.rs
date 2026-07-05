//! Tron Zero — client entry point.
//!
//! Connects to a lightyear server via UDP / raw connection, sends keyboard
//! inputs, predicts the local lightcycle, and renders the arena + players.

mod camera;
mod input;
mod render;

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::input::native::InputMarker;

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins);

    // --- Lightyear ---
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });

    // Register components + input type so protocol checksums match the server.
    shared::protocol::register_protocol(&mut app);

    // When the player entity arrives and Controlled is added (via ControlledBy
    // replication), tag it with InputMarker so lightyear's input pipeline picks
    // it up for buffering and transmission.
    app.add_observer(handle_controlled_spawn);

    // Spawn the client connection entity and connect to the server.
    app.add_systems(Startup, spawn_client);

    // Input: buffer key presses every frame, consume per fixed tick.
    app.init_resource::<input::PendingInput>();
    app.add_systems(Update, input::buffer_keyboard_input);
    app.add_systems(
        FixedPreUpdate,
        input::read_keyboard.in_set(InputSystems::WriteClientInputs),
    );

    // Simulation: same systems as the server, running on the predicted entity.
    app.add_systems(FixedUpdate, (shared::apply_turn, shared::move_players, shared::collide_with_arena).chain());

    // Rendering.
    app.add_systems(Startup, render::setup_camera);
    app.add_systems(
        Update,
        (
            render::draw_arena,
            render::draw_trails,
            render::draw_players,
            camera::follow_player,
        ),
    );

    app.run();
}

/// Spawn the client link entity and trigger connection.
fn spawn_client(mut commands: Commands) {
    let client = commands
        .spawn((
            RawClient,
            UdpIo::default(),
            LocalAddr(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                0,
            )),
            PeerAddr(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                5000,
            )),
            // Required by lightyear's prediction systems to initialize
            // PredictionResource (needed before replication data arrives).
            PredictionManager::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
    commands.trigger(LinkStart { entity: client });
}

/// When the predicted player entity receives `Controlled` (replicated from the
/// server's `ControlledBy`), insert `InputMarker` so lightyear's
/// `buffer_action_state` and `prepare_input_message` systems pick it up.
fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    players: Query<(&shared::Player, Option<&ControlledBy>), Without<InputMarker<shared::PlayerInput>>>,
    clients: Query<(), With<Client>>,
) {
    let entity = trigger.entity;
    let Ok((_, controlled_by)) = players.get(entity) else {
        return;
    };
    // Only tag if this entity is controlled by the local client.
    if let Some(cb) = controlled_by {
        if clients.get(cb.owner).is_err() {
            return;
        }
    }
    commands.entity(entity).insert(InputMarker::<shared::PlayerInput>::default());
}
