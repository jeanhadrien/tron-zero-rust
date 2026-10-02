//! Server systems: arena setup and connection lifecycle.

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use shared::protocol::{RespawnChannel, RespawnOutcome, RespawnReply, RespawnRequest};

#[derive(Component)]
#[require(shared::LifeGeneration)]
pub struct Human;

#[derive(Component)]
struct InitialSpawn;

#[derive(Resource, Default)]
pub struct PendingRespawns(Vec<(Entity, RespawnRequest)>);

/// Lightyear clears receivers in Last, including frames with no fixed tick.
/// Preserve requests until an authoritative fixed-tick transaction can run.
pub fn collect_respawn_requests(
    mut clients: Query<(Entity, &mut MessageReceiver<RespawnRequest>), With<Connected>>,
    mut pending: ResMut<PendingRespawns>,
) {
    for (owner, mut receiver) in &mut clients {
        pending
            .0
            .extend(receiver.receive().map(|request| (owner, request)));
    }
}

/// Spawn the arena entity with replication, then start the server listener.
pub fn spawn_server_arena_and_start(port: Res<crate::announce::UdpPort>, mut commands: Commands) {
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
                port.0,
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
pub fn on_client_connected(
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
        .insert((ReplicationSender, crate::session::SessionLease::default()));

    // Spawn allocation is serialized in FixedUpdate against live geometry.
    // Keep one controlled entity even when the arena has no safe space.
    commands.spawn((
        shared::Player,
        Human,
        InitialSpawn,
        shared::LifeGeneration::for_session(rand::random()),
        shared::PlayerId(remote_id.0.to_string()),
        shared::Velocity(Vec2::ZERO),
        shared::SpeedMult(0.0),
        shared::TargetSpeedMult(0.0),
        shared::Rubber(0.0),
        shared::PlayerColor(0x00FFCC),
        shared::IsAlive(false),
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
    ));
}

/// Drain queued per-link requests, deriving ownership from the link rather than
/// trusting a player identifier supplied by a client. Exclusive access makes
/// simultaneous allocations see each other's freshly reset trail and rider.
pub fn handle_respawns(world: &mut World) {
    let initial: Vec<_> = world
        .query_filtered::<(Entity, &ControlledBy, &shared::LifeGeneration), (With<Human>, With<InitialSpawn>)>()
        .iter(world)
        .map(|(entity, controlled, generation)| (entity, controlled.owner, generation.0))
        .collect();
    for (entity, owner, generation) in initial {
        world.entity_mut(entity).remove::<InitialSpawn>();
        let outcome = try_respawn(world, owner, RespawnRequest { generation });
        if outcome != RespawnOutcome::Accepted {
            warn!(
                ?owner,
                ?outcome,
                "Initial human spawn unavailable; manual retry required"
            );
        }
    }

    let requests = std::mem::take(&mut world.resource_mut::<PendingRespawns>().0);
    for (owner, request) in requests {
        let outcome = try_respawn(world, owner, request);
        match outcome {
            RespawnOutcome::Accepted => info!(
                ?owner,
                generation = request.generation,
                "Human respawn accepted"
            ),
            RespawnOutcome::NoSafeSpace => {
                warn!(?owner, "Human respawn denied: no safe spawn candidate")
            }
            RespawnOutcome::NotEligible => {
                debug!(?owner, "Ignoring ineligible or stale respawn request")
            }
        }
        if let Some(mut sender) = world.get_mut::<MessageSender<RespawnReply>>(owner) {
            sender.send::<RespawnChannel>(RespawnReply {
                generation: request.generation,
                outcome,
            });
        }
    }
}

fn try_respawn(world: &mut World, owner: Entity, request: RespawnRequest) -> RespawnOutcome {
    if world.get::<Connected>(owner).is_none() {
        return RespawnOutcome::NotEligible;
    }
    let eligible: Vec<_> = world
        .query_filtered::<(
            Entity,
            &ControlledBy,
            &shared::IsAlive,
            &shared::LifeGeneration,
        ), (With<Human>, With<shared::Player>)>()
        .iter(world)
        .filter(|(_, controlled, _, _)| controlled.owner == owner)
        .map(|(entity, _, alive, generation)| (entity, alive.0, generation.0))
        .collect();
    // An ambiguous/missing owner mapping must never create another rider.
    let [(entity, false, generation)] = eligible.as_slice() else {
        return RespawnOutcome::NotEligible;
    };
    if *generation != request.generation {
        return RespawnOutcome::NotEligible;
    }
    let entity = *entity;
    let generation = *generation;
    let Some(next_life) = shared::LifeGeneration(generation).next() else {
        return RespawnOutcome::NotEligible;
    };
    let mut arenas =
        world.query_filtered::<(&shared::ArenaSize, &shared::WallSegments), With<shared::Arena>>();
    let Ok((size, walls)) = arenas.single(world) else {
        return RespawnOutcome::NoSafeSpace;
    };
    let size = *size;
    let mut segments: Vec<_> = walls
        .0
        .iter()
        .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
        .collect();
    let mut riders = Vec::new();
    for (alive, position, trail) in world
        .query_filtered::<(&shared::IsAlive, &shared::Position, &shared::Trail), With<shared::Player>>()
        .iter(world)
    {
        if alive.0 {
            riders.push(position.0);
            segments.extend(trail.0.windows(2).map(|p| [p[0], p[1]]));
        }
    }
    let Some(spawn) = shared::spawn::find_safe_spawn(
        size,
        &segments,
        &riders,
        owner.to_bits().wrapping_add(generation as u64),
    ) else {
        return RespawnOutcome::NoSafeSpace;
    };
    world.entity_mut(entity).insert((
        shared::Position(spawn.position),
        shared::Direction(spawn.direction),
        shared::Velocity(spawn.direction * (shared::BASE_SPEED * shared::TICK_SECS * 1000.0)),
        shared::SpeedMult::base(),
        shared::TargetSpeedMult::default(),
        shared::Rubber::default(),
        shared::IsAlive(true),
        shared::Trail::new(spawn.position),
        shared::ShouldHandleDeath(true),
        shared::IsSliding(false),
        shared::IsColliding(false),
        shared::ActionState(shared::PlayerInput::None),
        next_life,
    ));
    RespawnOutcome::Accepted
}

/// When a client disconnects (Connected removed), find their player entity
/// via `ControlledBy.owner` and despawn it along with its trail component.
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

#[cfg(test)]
mod tests {
    use super::*;
    use shared::*;

    fn setup() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<PendingRespawns>();
        world.spawn((Arena, WallSegments(arena_walls(ARENA_WIDTH, ARENA_HEIGHT))));
        let owner = world.spawn((RemoteId(PeerId::Local(1)), Connected)).id();
        let player = world
            .spawn((
                Player,
                Human,
                LifeGeneration(7),
                IsAlive(false),
                ControlledBy {
                    owner,
                    lifetime: Lifetime::SessionBased,
                },
            ))
            .id();
        (world, owner, player)
    }

    #[test]
    fn only_connected_owner_of_dead_human_can_respawn_current_life() {
        let (mut world, owner, player) = setup();
        let stranger = world.spawn((RemoteId(PeerId::Local(2)), Connected)).id();
        let request = RespawnRequest { generation: 7 };
        assert_eq!(
            try_respawn(&mut world, stranger, request),
            RespawnOutcome::NotEligible
        );
        assert_eq!(
            try_respawn(&mut world, owner, RespawnRequest { generation: 6 }),
            RespawnOutcome::NotEligible
        );
        world.get_mut::<IsAlive>(player).unwrap().0 = true;
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::NotEligible
        );
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        world.entity_mut(owner).remove::<Connected>();
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::NotEligible
        );
        world.entity_mut(owner).insert(Connected);
        world.entity_mut(player).remove::<Human>();
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::NotEligible
        );
    }

    #[test]
    fn respawn_resets_same_entity_and_duplicate_is_ineligible_even_after_next_death() {
        let (mut world, owner, player) = setup();
        world.entity_mut(player).insert((
            Position(Vec2::splat(999.0)),
            Direction(Vec2::NEG_Y),
            Velocity(Vec2::splat(9000.0)),
            SpeedMult(20.0),
            TargetSpeedMult(30.0),
            Rubber(0.0),
            IsSliding(true),
            IsColliding(true),
            ShouldHandleDeath(false),
            Trail(vec![Vec2::ZERO, Vec2::X]),
            ActionState(PlayerInput::TurnLeftFor(7)),
        ));
        let request = RespawnRequest { generation: 7 };
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::Accepted
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<Player>>()
                .iter(&world)
                .count(),
            1
        );
        assert_eq!(world.get::<ControlledBy>(player).unwrap().owner, owner);
        assert_eq!(world.get::<LifeGeneration>(player).unwrap().0, 8);
        assert!(world.get::<IsAlive>(player).unwrap().0);
        assert!(world.get::<ShouldHandleDeath>(player).unwrap().0);
        assert!(!world.get::<IsSliding>(player).unwrap().0);
        assert!(!world.get::<IsColliding>(player).unwrap().0);
        assert_eq!(world.get::<Rubber>(player).unwrap().0, BASE_RUBBER);
        assert_eq!(world.get::<SpeedMult>(player).unwrap().0, 1.0);
        assert_eq!(world.get::<TargetSpeedMult>(player).unwrap().0, 1.0);
        assert_eq!(
            world.get::<ActionState<PlayerInput>>(player).unwrap().0,
            PlayerInput::None
        );
        let pos = world.get::<Position>(player).unwrap().0;
        assert_eq!(world.get::<Trail>(player).unwrap().0, Trail::new(pos).0);
        assert_eq!(
            world.get::<Velocity>(player).unwrap().0,
            world.get::<Direction>(player).unwrap().0 * 3000.0
        );
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::NotEligible
        );
        world.get_mut::<IsAlive>(player).unwrap().0 = false;
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::NotEligible
        );
    }

    #[test]
    fn no_safe_space_leaves_dead_state_and_identity_unchanged() {
        let (mut world, owner, player) = setup();
        let arena = world
            .query_filtered::<Entity, With<Arena>>()
            .single(&world)
            .unwrap();
        world.entity_mut(arena).insert(ArenaSize {
            width: 150.0,
            height: 150.0,
        });
        assert_eq!(
            try_respawn(&mut world, owner, RespawnRequest { generation: 7 }),
            RespawnOutcome::NoSafeSpace
        );
        assert!(!world.get::<IsAlive>(player).unwrap().0);
        assert_eq!(world.get::<LifeGeneration>(player).unwrap().0, 7);
        assert_eq!(
            world
                .query_filtered::<Entity, With<Player>>()
                .iter(&world)
                .count(),
            1
        );
    }

    #[test]
    fn allocations_observe_freshly_spawned_riders() {
        let (mut world, owner, player) = setup();
        let owner2 = world.spawn((RemoteId(PeerId::Local(2)), Connected)).id();
        let player2 = world
            .spawn((
                Player,
                Human,
                ControlledBy {
                    owner: owner2,
                    lifetime: Lifetime::SessionBased,
                },
            ))
            .id();
        assert_eq!(
            try_respawn(&mut world, owner, RespawnRequest { generation: 7 }),
            RespawnOutcome::Accepted
        );
        assert_eq!(
            try_respawn(&mut world, owner2, RespawnRequest { generation: 0 }),
            RespawnOutcome::Accepted
        );
        let pos1 = world.get::<Position>(player).unwrap().0;
        let pos2 = world.get::<Position>(player2).unwrap().0;
        assert!(pos1.distance(pos2) >= shared::spawn::SPAWN_CLEARANCE);
    }

    #[test]
    fn queued_request_cannot_outlive_its_connected_session() {
        let (mut world, owner, player) = setup();
        world
            .resource_mut::<PendingRespawns>()
            .0
            .push((owner, RespawnRequest { generation: 7 }));
        // A request retained across a render frame is still checked at the
        // fixed tick, not authorized when it was initially received.
        world.entity_mut(owner).remove::<Connected>();
        handle_respawns(&mut world);
        assert!(world.resource::<PendingRespawns>().0.is_empty());
        assert!(!world.get::<IsAlive>(player).unwrap().0);
        assert_eq!(world.get::<LifeGeneration>(player).unwrap().0, 7);
    }

    #[test]
    fn previous_connection_life_cannot_match_reconnected_human() {
        let (mut world, owner, player) = setup();
        let old_life = LifeGeneration::for_session(123).next().unwrap();
        let new_life = LifeGeneration::for_session(456).next().unwrap();
        world.entity_mut(player).insert(new_life);
        assert_eq!(
            try_respawn(
                &mut world,
                owner,
                RespawnRequest {
                    generation: old_life.0
                }
            ),
            RespawnOutcome::NotEligible
        );
        assert!(!world.get::<IsAlive>(player).unwrap().0);
        assert_eq!(world.get::<LifeGeneration>(player).unwrap().0, new_life.0);
        assert_eq!(
            PlayerInput::TurnLeftFor(old_life.0).eligible_turn(Some(&new_life)),
            PlayerInput::None
        );
        assert_eq!(
            try_respawn(
                &mut world,
                owner,
                RespawnRequest {
                    generation: new_life.0
                }
            ),
            RespawnOutcome::Accepted
        );
        assert_eq!(
            world.get::<LifeGeneration>(player).unwrap().0,
            new_life.0 + 1
        );
    }

    #[test]
    fn exhausted_life_counter_is_rejected_without_changing_session_nonce() {
        let (mut world, owner, player) = setup();
        let generation = LifeGeneration::for_session(123).0 | u64::MAX as u128;
        world.entity_mut(player).insert(LifeGeneration(generation));
        assert_eq!(
            try_respawn(&mut world, owner, RespawnRequest { generation }),
            RespawnOutcome::NotEligible
        );
        assert!(!world.get::<IsAlive>(player).unwrap().0);
        assert_eq!(world.get::<LifeGeneration>(player).unwrap().0, generation);
    }

    #[test]
    fn ambiguous_owner_mapping_does_not_reset_or_create_any_rider() {
        let (mut world, owner, player) = setup();
        let duplicate = world
            .spawn((
                Player,
                Human,
                LifeGeneration(7),
                ControlledBy {
                    owner,
                    lifetime: Lifetime::SessionBased,
                },
            ))
            .id();
        assert_eq!(
            try_respawn(&mut world, owner, RespawnRequest { generation: 7 }),
            RespawnOutcome::NotEligible
        );
        for entity in [player, duplicate] {
            assert!(!world.get::<IsAlive>(entity).unwrap().0);
            assert_eq!(world.get::<LifeGeneration>(entity).unwrap().0, 7);
        }
        assert_eq!(
            world
                .query_filtered::<Entity, With<Player>>()
                .iter(&world)
                .count(),
            2
        );
    }
}
