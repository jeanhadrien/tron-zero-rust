//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients,
//! and processes client inputs via lightyear.

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear::prelude::*;
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
    app.add_observer(on_client_connected);

    // Simulation systems in FixedUpdate.
    app.add_systems(FixedUpdate, (shared::apply_turn, shared::move_players).chain());

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(Startup, spawn_server_arena_and_start);

    app.run();
}

/// Spawn the arena entity with replication, then start the server listener.
fn spawn_server_arena_and_start(mut commands: Commands) {
    // Arena — replicated once to all clients.
    let size = shared::ArenaSize::default();
    commands.spawn((
        shared::Arena,
        size,
        shared::WallSegments(shared::arena_walls(size.width, size.height)),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // Server link entity.
    let server = commands
        .spawn((
            RawServer,
            ServerUdpIo::default(),
            LocalAddr(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
                5000,
            )),
        ))
        .id();
    commands.trigger(Start { entity: server });
}

/// When a new client's link-of entity becomes connected, set up replication
/// on the link and spawn a player entity for that client.
///
/// Observes `ClientOf` (inserted last in the connection bundle) so that
/// `Connected`, `RemoteId`, etc. are guaranteed present by the time we run.
fn on_client_connected(
    trigger: On<Add, ClientOf>,
    query: Query<&RemoteId, With<Connected>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };

    // Enable replication on this client's link entity.
    commands
        .entity(trigger.entity)
        .insert(ReplicationSender);

    // Spawn a player for this client.
    let player = commands
        .spawn((
            shared::Player,
            shared::PlayerId(remote_id.0.to_string()),
            shared::Position(Vec2::ZERO),
            shared::Direction(Vec2::new(1.0, 0.0)),
            shared::Velocity(Vec2::ZERO),
            shared::SpeedMult::base(),
            shared::PlayerColor(0x00FFCC),
            shared::IsAlive(true),
            shared::TrailPointCount(1),
            shared::TrailPointNextOrder(1),
            // Replicate to all clients.
            Replicate::to_clients(NetworkTarget::All),
            // The owning client predicts this entity.
            PredictionTarget::to_clients(NetworkTarget::Single(remote_id.0)),
            // All other clients interpolate it.
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(remote_id.0)),
        ))
        .id();

    // Initial trail point at spawn location.
    commands.spawn((
        shared::TrailPoint,
        shared::TrailPointOrder(0),
        shared::Position(Vec2::ZERO),
        shared::Direction(Vec2::new(1.0, 0.0)),
        ChildOf(player),
    ));
}
