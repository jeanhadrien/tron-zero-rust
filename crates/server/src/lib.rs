//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients and executes
//! their turn commands. The library holds all gameplay wiring so integration
//! tests (`tron-zero-nettest`) run the same app as `main.rs`.

pub mod admission;
pub mod announce;
pub mod bot;
pub mod budget;
pub mod chat;
pub mod elimination;
pub mod look;
pub mod probe;
pub mod ranked;
pub mod session;
pub mod systems;
pub mod turns;
pub mod web;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::server::*;
use lightyear::prelude::{LinkSystems, MessageSystems};
use lightyear_transport::plugin::TransportSystems;

/// Everything a server needs except bots, the manager announcer and logging:
/// Lightyear, protocol, connection lifecycle, respawns, turns and simulation.
/// Expects `MinimalPlugins` to be present; `port` is the UDP port to bind.
/// Browser players need [`add_web`] on top.
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
    // Discovery probes are answered before the transport parses them as packets.
    app.add_systems(
        PreUpdate,
        probe::answer_probes
            .after(LinkSystems::Receive)
            .before(TransportSystems::Receive),
    );
    app.add_systems(
        PreUpdate,
        (
            systems::collect_respawn_requests,
            turns::collect_turn_commands,
            chat::collect_chat,
            session::receive_session_requests,
            // Before the hello is answered: a look sent with it is on the link
            // by the time the rider is spawned.
            look::collect_looks.before(admission::handle_hello),
            admission::handle_hello,
            admission::discard_late_handshakes.after(admission::handle_hello),
        )
            .after(MessageSystems::Receive),
    );
    app.add_systems(
        PostUpdate,
        (
            // One coalesced ack per link per frame, queued before the
            // transport serializes messages.
            turns::send_turn_acks.before(MessageSystems::Send),
            session::close_sessions.after(lightyear::prelude::LinkSystems::Send),
        ),
    );
    // Links that die without a clean leave are never despawned by Lightyear.
    // Unordered against Lightyear's own `Last` disconnect on purpose: links it
    // is closing carry `Disconnecting` and are skipped, and the despawn is a
    // `try_despawn`, so either order is safe.
    app.add_systems(Last, systems::reap_dead_links);

    // `apply_turn_commands` runs after Lightyear's `UpdateActionState`
    // (FixedPreUpdate) and overwrites its native-input fallback for human
    // riders, so only sequenced turn commands can turn a human.
    app.insert_resource(shared::SimulationRole::Server);
    // Only the server collects deaths: simulate_players re-runs on client
    // rollback, so a client must never hold a DeathLog.
    app.init_resource::<shared::DeathLog>();
    app.init_resource::<elimination::Eliminations>();
    app.add_systems(
        FixedUpdate,
        (
            systems::handle_respawns,
            turns::apply_turn_commands,
            shared::simulate_players,
            // Reads the end-of-tick state before anything is despawned.
            elimination::resolve_deaths,
            // Only ranked servers report results (see `ranked`).
            ranked::forward_eliminations.run_if(resource_exists::<ranked::RankedMode>),
        )
            .chain(),
    );
    app.add_systems(
        Update,
        ranked::publish_roster.run_if(resource_exists::<ranked::RankedMode>),
    );

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(Startup, systems::spawn_server_arena_and_start);
}

/// Also accept WebTransport sessions (browser players) on `endpoint.port`.
/// Call after [`add_core`]; the listener opens with the server at startup.
pub fn add_web(app: &mut App, endpoint: web::WebEndpoint) {
    web::add_observers(app);
    app.insert_resource(endpoint);
}
