//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients,
//! and processes client inputs via lightyear.

mod bot;
mod session;
mod systems;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::server::*;

fn main() {
    let mut app = App::new();

    // Headless: no window, event-driven I/O loop.
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    // Required by lightyear replication internals.
    app.add_plugins(bevy::state::app::StatesPlugin);

    // --- Lightyear ---
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });

    // Register components + input type so protocol checksums match.
    shared::protocol::register_protocol(&mut app);

    // New client observer — runs after RawConnectionPlugin sets up Connected + ClientOf.
    app.add_observer(systems::on_client_connected);
    // Disconnect observer — cleans up zombie player entities.
    app.add_observer(systems::on_client_disconnected);
    app.init_resource::<systems::PendingRespawns>();
    app.add_systems(
        PreUpdate,
        systems::collect_respawn_requests.after(lightyear::prelude::MessageSystems::Receive),
    );
    app.add_systems(
        PreUpdate,
        session::receive_session_requests.after(lightyear::prelude::MessageSystems::Receive),
    );
    app.add_systems(
        PostUpdate,
        session::close_sessions.after(lightyear::prelude::LinkSystems::Send),
    );

    // Simulation systems in FixedUpdate.
    app.insert_resource(shared::SimulationRole::Server);
    app.add_systems(
        FixedUpdate,
        (
            systems::handle_respawns,
            bot::bot_brain_input,
            shared::simulate_players,
            bot::bot_spawner,
        )
            .chain(),
    );

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(
        Startup,
        (systems::spawn_server_arena_and_start, bot::setup_bots).chain(),
    );

    app.run();
}
