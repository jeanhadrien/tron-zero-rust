//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients and executes
//! their turn commands. The library holds all gameplay wiring so integration
//! tests (`tron-zero-nettest`) run the same app as `main.rs`.

pub mod admission;
pub mod announce;
pub mod bot;
pub mod session;
pub mod systems;
pub mod turns;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::MessageSystems;
use lightyear::prelude::server::*;

/// Everything a server needs except bots, the manager announcer and logging:
/// Lightyear, protocol, connection lifecycle, respawns, turns and simulation.
/// Expects `MinimalPlugins` to be present; `port` is the UDP port to bind.
pub fn add_core(app: &mut App, port: u16) {
    // Required by lightyear replication internals.
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });
    // Register components + input type so protocol checksums match.
    shared::protocol::register_protocol(app);

    app.insert_resource(announce::UdpPort(port));
    // New client observer — runs after RawConnectionPlugin sets up Connected + ClientOf.
    app.add_observer(systems::on_client_connected);
    // Disconnect observer — cleans up zombie player entities.
    app.add_observer(systems::on_client_disconnected);
    app.init_resource::<systems::PendingRespawns>();
    // main.rs inserts the configured cap first; the default only serves tests.
    app.init_resource::<admission::MaxPlayers>();
    app.add_systems(
        PreUpdate,
        (
            systems::collect_respawn_requests,
            turns::collect_turn_commands,
            session::receive_session_requests,
            admission::handle_hello,
        )
            .after(MessageSystems::Receive),
    );
    app.add_systems(
        PostUpdate,
        session::close_sessions.after(lightyear::prelude::LinkSystems::Send),
    );

    // `apply_turn_commands` runs after Lightyear's `UpdateActionState`
    // (FixedPreUpdate) and overwrites its native-input fallback for human
    // riders, so only sequenced turn commands can turn a human.
    app.insert_resource(shared::SimulationRole::Server);
    app.add_systems(
        FixedUpdate,
        (
            systems::handle_respawns,
            turns::apply_turn_commands,
            shared::simulate_players,
        )
            .chain(),
    );

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(Startup, systems::spawn_server_arena_and_start);
}
