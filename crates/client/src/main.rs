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

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins);

    // --- Lightyear ---
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });

    // Register components + input type so protocol checksums match the server.
    shared::protocol::register_protocol(&mut app);

    // Spawn the client connection entity and connect to the server.
    app.add_systems(Startup, spawn_client);

    // Input: keyboard → ActionState, per fixed tick.
    app.add_systems(
        FixedPreUpdate,
        input::read_keyboard.in_set(InputSystems::WriteClientInputs),
    );

    // Simulation: same systems as the server, running on the predicted entity.
    app.add_systems(FixedUpdate, (shared::apply_turn, shared::move_players).chain());

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
