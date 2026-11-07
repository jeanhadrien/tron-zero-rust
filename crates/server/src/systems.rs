//! Server systems: arena setup, connection lifecycle, respawns.

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use shared::protocol::{RespawnChannel, RespawnOutcome, RespawnReply, RespawnRequest};

use crate::admission::{AwaitingHello, MAX_UNADMITTED_LINKS};
use crate::budget::{LinkBudget, Meter};
use crate::look::LinkLook;
use crate::session::SessionLease;

#[derive(Component)]
#[require(shared::LifeGeneration)]
pub struct Human;

#[derive(Component)]
struct InitialSpawn;

#[derive(Resource, Default)]
pub struct PendingRespawns(Vec<(Entity, RespawnRequest)>);

/// Lightyear clears receivers in Last, including frames with no fixed tick.
/// Preserve requests until an authoritative fixed-tick transaction can run.
///
/// Each request is answered on a reliable channel, so a link sending more than
/// a client ever should is closed instead of being allowed to grow that buffer.
pub fn collect_respawn_requests(
    time: Res<Time<Real>>,
    mut clients: Query<
        (
            Entity,
            &mut MessageReceiver<RespawnRequest>,
            &mut LinkBudget,
            &mut SessionLease,
        ),
        With<Connected>,
    >,
    mut pending: ResMut<PendingRespawns>,
) {
    let now = time.elapsed_secs_f64();
    for (owner, mut receiver, mut budget, mut lease) in &mut clients {
        for request in receiver.receive() {
            if !budget.spend(Meter::RespawnRequests, 1, now) {
                warn!(?owner, "Respawn request flood; closing the link");
                lease.close();
                break;
            }
            pending.0.push((owner, request));
        }
    }
}

/// Spawn the arena entity with replication, then start the server listener
/// (and the WebTransport one, when configured).
pub fn spawn_server_arena_and_start(
    port: Res<crate::announce::UdpPort>,
    web: Option<Res<crate::web::WebEndpoint>>,
    mut commands: Commands,
) {
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
    // Browser players join the same server entity over WebTransport.
    if let Some(web) = web {
        crate::web::open_listener(&mut commands, server, &web);
    }
}

/// When a new client's link-of entity becomes connected, set up its session
/// state. Nothing is replicated to it and no rider is spawned yet: both happen
/// once the client's hello is accepted (`admission::handle_hello`), because a
/// raw UDP "connection" exists as soon as any packet arrives from an address.
/// Past [`MAX_UNADMITTED_LINKS`] links waiting for a hello, new ones are closed.
///
/// Observes `ClientOf` (inserted last in the connection bundle) so that
/// `Connected`, `RemoteId`, etc. are guaranteed present by the time we run.
pub fn on_client_connected(
    trigger: On<Add, ClientOf>,
    awaiting: Query<(), With<AwaitingHello>>,
    mut commands: Commands,
) {
    let mut lease = SessionLease::default();
    if awaiting.iter().count() >= MAX_UNADMITTED_LINKS {
        warn!(link = ?trigger.entity, "Too many links awaiting a hello; closing the new one");
        lease.close();
    }
    commands.entity(trigger.entity).insert((
        lease,
        LinkBudget::default(),
        crate::turns::TurnQueue::default(),
        LinkLook::default(),
        AwaitingHello::default(),
    ));
}

/// Spawn the controlled rider for an admitted client link. Spawn allocation
/// is serialized in FixedUpdate against live geometry (`InitialSpawn`); the
/// rider exists even when the arena has no safe space yet.
pub(crate) fn spawn_human(
    commands: &mut Commands,
    link: Entity,
    peer: PeerId,
    name: String,
    look: LinkLook,
) -> Entity {
    // (Split insert: the 15-component tuple Bundle limit is already reached.)
    commands
        .spawn((
            shared::Player,
            Human,
            InitialSpawn,
            shared::LifeGeneration::for_session(rand::random()),
            shared::PlayerId(peer.to_string()),
            shared::Velocity(Vec2::ZERO),
            shared::SpeedMult(0.0),
            shared::TargetSpeedMult(0.0),
            shared::Rubber(0.0),
            shared::IsAlive(false),
            // Replicate to all clients.
            Replicate::to_clients(NetworkTarget::All),
            // The owning client predicts this entity.
            PredictionTarget::to_clients(NetworkTarget::Single(peer)),
            // All other clients interpolate it.
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(peer)),
            // Routes this client's inputs to this entity's ActionState.
            ControlledBy {
                owner: link,
                lifetime: Lifetime::SessionBased,
            },
        ))
        .insert((
            shared::DisplayName(name),
            shared::PlayerColor(look.color),
            look.shape,
        ))
        .id()
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

/// Despawns client links whose transport died underneath us (a browser tab
/// closed or throttled without a `Leave`). Lightyear only despawns links that
/// carry `Disconnecting`, and our lease timeout only sees `Connected` links, so
/// a link that went `Unlinked`/`Disconnected` on its own would otherwise stay
/// forever, still replicated to by replicon and buffering messages.
#[allow(clippy::type_complexity)]
pub fn reap_dead_links(
    dead: Query<
        Entity,
        (
            With<LinkOf>,
            Or<(With<Unlinked>, With<Disconnected>)>,
            Without<lightyear::connection::client::Disconnecting>,
        ),
    >,
    mut commands: Commands,
) {
    for link in &dead {
        // Lightyear may despawn the same link in this `Last` pass.
        commands.entity(link).try_despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::*;

    #[test]
    fn dead_links_are_reaped_and_live_ones_are_not() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        // A relationship whose target is missing removes itself, so use a real one.
        let server = world.spawn_empty().id();
        let live = world
            .spawn((LinkOf { server }, RemoteId(PeerId::Local(1)), Connected))
            .id();
        let unlinked = world.spawn((LinkOf { server }, Unlinked::default())).id();
        let dropped = world
            .spawn((LinkOf { server }, Disconnected::default()))
            .id();
        let closing = world
            .spawn((
                LinkOf { server },
                Disconnected::default(),
                lightyear::connection::client::Disconnecting,
            ))
            .id();
        world.run_system_once(reap_dead_links).unwrap();
        assert!(world.get_entity(live).is_ok());
        assert!(
            world.get_entity(closing).is_ok(),
            "Lightyear's own path owns it"
        );
        assert!(world.get_entity(unlinked).is_err());
        assert!(world.get_entity(dropped).is_err());
    }

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
            PlayerColor(0x123456),
            RiderShape::Hexagon,
        ));
        let request = RespawnRequest { generation: 7 };
        assert_eq!(
            try_respawn(&mut world, owner, request),
            RespawnOutcome::Accepted
        );
        assert_eq!(world.get::<PlayerColor>(player).unwrap().0, 0x123456);
        assert_eq!(
            *world.get::<RiderShape>(player).unwrap(),
            RiderShape::Hexagon
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
            world.get::<Direction>(player).unwrap().0
                * (shared::BASE_SPEED * shared::TICK_SECS * 1000.0)
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
