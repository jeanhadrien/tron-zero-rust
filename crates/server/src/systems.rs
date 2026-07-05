//! Server systems: arena setup, connection lifecycle, trail replication.

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use lightyear::prelude::server::*;
use lightyear::prelude::*;

/// Spawn the arena entity with replication, then start the server listener.
pub fn spawn_server_arena_and_start(mut commands: Commands) {
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
            LocalAddr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 5000)),
        ))
        .id();
    commands.trigger(Start { entity: server });
}

/// When a new client's link-of entity becomes connected, set up replication
/// on the link and spawn a player entity for that client.
///
/// Observes `ClientOf` (inserted last in the connection bundle) so that
/// `Connected`, `RemoteId`, etc. are guaranteed present by the time we run.
pub fn on_client_connected(
    trigger: On<Add, ClientOf>,
    query: Query<&RemoteId, With<Connected>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.entity) else {
        return;
    };

    // Enable replication on this client's link entity.
    commands.entity(trigger.entity).insert(ReplicationSender);

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
            // Routes this client's inputs to this entity's ActionState.
            ControlledBy {
                owner: trigger.entity,
                lifetime: Lifetime::SessionBased,
            },
        ))
        .id();

    // Initial trail point at spawn location.
    commands.spawn((
        shared::TrailPoint,
        shared::TrailPointOrder(0),
        shared::Position(Vec2::ZERO),
        shared::Direction(Vec2::new(1.0, 0.0)),
        ChildOf(player),
        Replicate::to_clients(NetworkTarget::All),
    ));
}

/// When a client disconnects (Connected removed), find their player entity
/// via `ControlledBy.owner` and despawn it along with its TrailPoint children.
pub fn on_client_disconnected(
    trigger: On<Remove, Connected>,
    players: Query<(Entity, &ControlledBy)>,
    mut commands: Commands,
) {
    for (entity, controlled_by) in &players {
        if controlled_by.owner == trigger.entity {
            commands.entity(entity).despawn();
        }
    }
}

/// Ensure trail points spawned by `apply_turn` carry `Replicate` so lightyear
/// sends them to clients. Runs after `apply_turn` each tick.
pub fn mark_trail_points_for_replication(
    new_trail_points: Query<Entity, (With<shared::TrailPoint>, Without<Replicate>)>,
    mut commands: Commands,
) {
    for entity in &new_trail_points {
        commands
            .entity(entity)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}
