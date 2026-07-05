//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients,
//! and processes client inputs via lightyear.

mod systems;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::server::*;

fn main() {
    let mut app = App::new();

    // Headless: no window, event-driven I/O loop.
    app.add_plugins(MinimalPlugins);
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

    // Simulation systems in FixedUpdate.
    app.add_systems(
        FixedUpdate,
        (
            shared::apply_turn,
            shared::move_players,
            shared::collide_with_arena,
        )
            .chain(),
    );
    app.add_systems(
        FixedUpdate,
        systems::mark_trail_points_for_replication.after(shared::apply_turn),
    );

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(Startup, systems::spawn_server_arena_and_start);

    app.run();
}
