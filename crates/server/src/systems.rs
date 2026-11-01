//! Server systems: arena setup, connection lifecycle, one-shot turns.

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use shared::protocol::{
    RespawnChannel, RespawnOutcome, RespawnReply, RespawnRequest, TURN_QUEUE_CAP,
    TURN_QUEUE_TTL_TICKS, TurnAck, TurnChannel, TurnCommand, TurnOutcome, TurnRejectReason,
};

#[derive(Component)]
#[require(shared::LifeGeneration)]
pub struct Human;

#[derive(Component)]
struct InitialSpawn;

#[derive(Resource, Default)]
pub struct PendingRespawns(Vec<(Entity, RespawnRequest)>);

/// One-shot turn delivery (phantom-turn fix).
///
/// Lightyear 0.28's `update_action_state` applies
/// `input_buffer.get_predict(tick)`, which repeats the last TurnLeft/TurnRight
/// when input is missing. This queue makes missing input mean "no turn":
/// `collect_turn_commands` (PreUpdate, after `MessageSystems::Receive`)
/// buffers sequenced `TurnCommand`s per owning link, and `apply_turn_commands`
/// (FixedUpdate, after Lightyear's `UpdateActionState`, before
/// `simulate_players`) overwrites `ActionState` for client-owned Human riders
/// from the queue — at most one command per player per tick, in seq order.
/// Missing input writes `None`. Duplicates, stale-life, dead-rider, expired
/// and over-cap commands are acked as rejected and never execute.
///
/// BOUNDED LATE EXECUTION, not strict intended-tick rejection: with zero input
/// lead every turn would be "late" under any RTT, so commands execute on the
/// next available tick(s) after receipt. The server never rewinds, never
/// relocates corners, never reverses deaths. Bots keep writing `ActionState`
/// directly and are skipped. `LifeGeneration` tagging makes old lives stale;
/// the queue is also cleared explicitly on death/respawn. Disconnect despawns
/// the rider entity, and all turn state lives on link/rider entities — no
/// resource-level queue can outlive the link.
///
/// ROLLBACK: the server never rolls back, so the queue needs no rollback
/// handling. The client replays the same ordered press sequence through its
/// native `InputBuffer` history; dedup by `(generation, seq)` means duplicate
/// packets never double-fire.
#[derive(Component, Default, Debug)]
pub struct TurnQueue {
    /// Fifo of accepted commands, in seq order.
    pub queue: std::collections::VecDeque<QueuedTurn>,
    /// Highest seq ever accepted per generation (dedup + ordering).
    pub accepted: std::collections::HashMap<u128, u64>,
    /// Seqs already rejected as duplicates (so a resent duplicate still acks).
    pub rejected_dup: std::collections::HashSet<(u128, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedTurn {
    pub seq: u64,
    pub direction: shared::protocol::TurnDirection,
    pub generation: u128,
    pub client_tick: u32,
    /// Server `LocalTimeline` tick at receipt (TTL + diagnostics).
    pub received_tick: u32,
}

impl TurnQueue {
    /// Validate one command at receipt time. Returns the immediate outcome for
    /// hard rejects (`None` means queued, ack deferred until execution so the
    /// ack can report the execution tick).
    ///
    /// Requires non-decreasing `(generation, seq)` per generation, which
    /// `collect_turn_commands` guarantees by sorting each batch (the transport
    /// is ordered-reliable, so batches are ordered too). Under that
    /// precondition the accepted-seq watermark is the duplicate guard: any seq
    /// at or below it was already queued or executed.
    fn offer(&mut self, cmd: TurnCommand, received_tick: u32) -> Option<TurnOutcome> {
        if self.rejected_dup.contains(&(cmd.generation, cmd.seq))
            || self
                .accepted
                .get(&cmd.generation)
                .is_some_and(|&max| cmd.seq <= max)
            || self
                .queue
                .iter()
                .any(|q| q.generation == cmd.generation && q.seq == cmd.seq)
        {
            return Some(TurnOutcome::Rejected {
                reason: TurnRejectReason::Duplicate,
            });
        }
        if self.queue.len() >= TURN_QUEUE_CAP {
            return Some(TurnOutcome::Rejected {
                reason: TurnRejectReason::QueueFull,
            });
        }
        // Keep the queue in seq order per generation on insert so execution is
        // a plain pop_front even under transport reorder.
        let pos = self
            .queue
            .iter()
            .position(|q| {
                q.generation > cmd.generation || (q.generation == cmd.generation && q.seq > cmd.seq)
            })
            .unwrap_or(self.queue.len());
        self.queue.insert(
            pos,
            QueuedTurn {
                seq: cmd.seq,
                direction: cmd.direction,
                generation: cmd.generation,
                client_tick: cmd.client_tick,
                received_tick,
            },
        );
        self.accepted
            .entry(cmd.generation)
            .and_modify(|max| *max = (*max).max(cmd.seq))
            .or_insert(cmd.seq);
        None
    }

    /// Drop commands older than ~1s, returning their seqs for `Expired` acks.
    fn expire(&mut self, now_tick: u32) -> Vec<(u64, u128)> {
        let mut expired = Vec::new();
        self.queue.retain(|q| {
            let age = now_tick.wrapping_sub(q.received_tick);
            if age > TURN_QUEUE_TTL_TICKS {
                expired.push((q.seq, q.generation));
                false
            } else {
                true
            }
        });
        expired
    }
}

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

/// Buffer sequenced `TurnCommand`s onto the owning link's `TurnQueue`.
///
/// Ownership comes from the link (no entity id in the message). Hard rejects
/// (duplicate, over-cap) ack immediately; queued commands ack at execution so
/// `Applied` can carry the execution tick. World-exclusive so link queue
/// insert + receiver drain + sender write stay in one transaction.
pub fn collect_turn_commands(world: &mut World) {
    let now_tick: u32 = world
        .get_resource::<LocalTimeline>()
        .map(|t| t.tick().0)
        .unwrap_or(0);
    // Drain receivers without holding borrows across queue writes.
    let mut batches: Vec<(Entity, Vec<TurnCommand>)> = Vec::new();
    {
        let mut query =
            world.query_filtered::<(Entity, &mut MessageReceiver<TurnCommand>), With<Connected>>();
        for (owner, mut receiver) in query.iter_mut(world) {
            let cmds: Vec<TurnCommand> = receiver.receive().collect();
            if !cmds.is_empty() {
                batches.push((owner, cmds));
            }
        }
    }
    for (owner, mut pending) in batches {
        // Sort by (generation, seq): ordered-reliable delivery is already
        // ordered, but this keeps execution order well-defined regardless.
        pending.sort_by_key(|cmd| (cmd.generation, cmd.seq));
        if world.get_entity(owner).is_err() {
            continue;
        }
        if world.get::<TurnQueue>(owner).is_none() {
            world.entity_mut(owner).insert(TurnQueue::default());
        }
        let mut immediates: Vec<TurnAck> = Vec::new();
        let mut received: Vec<(u64, shared::protocol::TurnDirection, u128, u32)> = Vec::new();
        if let Some(mut queue) = world.get_mut::<TurnQueue>(owner) {
            for cmd in pending {
                tracing::debug!(
                    ?owner,
                    seq = cmd.seq,
                    ?cmd.direction,
                    generation = cmd.generation,
                    client_tick = cmd.client_tick,
                    received_tick = now_tick,
                    queue_depth = queue.queue.len(),
                    "Turn command received"
                );
                match queue.offer(cmd, now_tick) {
                    Some(outcome) => immediates.push(TurnAck {
                        seq: cmd.seq,
                        outcome,
                    }),
                    None => {
                        received.push((cmd.seq, cmd.direction, cmd.generation, cmd.client_tick));
                    }
                }
            }
        }
        for (seq, direction, generation, client_tick) in received {
            tracing::debug!(
                ?owner,
                seq,
                ?direction,
                generation,
                client_tick,
                received_tick = now_tick,
                "Turn command queued"
            );
        }
        for ack in immediates {
            if let TurnOutcome::Rejected { reason } = ack.outcome {
                tracing::debug!(
                    ?owner,
                    seq = ack.seq,
                    ?reason,
                    received_tick = now_tick,
                    "Turn command rejected at receipt"
                );
            }
            ack_on(world, owner, ack);
        }
    }
}

/// Execute at most one queued turn per Human rider per fixed tick, in seq
/// order, then overwrite `ActionState` (or `None` when nothing is due).
///
/// Runs AFTER Lightyear's `InputSystems::UpdateActionState` (FixedPreUpdate)
/// and BEFORE `simulate_players` (FixedUpdate chain), so the native-input
/// `get_predict` fallback is irrelevant on the server. Skips bot entities
/// (any `Player` without link-owned `ControlledBy`, incl. `BotBrain`) — bots
/// keep writing `ActionState` directly. Clears the queue when the rider is
/// dead or its generation moved on, so stale-life commands never linger.
pub fn apply_turn_commands(world: &mut World) {
    let now_tick: u32 = world
        .get_resource::<LocalTimeline>()
        .map(|t| t.tick().0)
        .unwrap_or(0);
    // Snapshot ownership: exactly one dead/alive Human rider per connected
    // link, mirroring `try_respawn`'s ownership pattern.
    struct Rider {
        link: Entity,
        entity: Entity,
        alive: bool,
        generation: u128,
    }
    let mut riders: Vec<Rider> = Vec::new();
    let mut ambiguous: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    {
        let mut query = world.query_filtered::<(
            Entity,
            &ControlledBy,
            &shared::IsAlive,
            &shared::LifeGeneration,
        ), (With<Human>, With<shared::Player>)>();
        for (entity, controlled, alive, generation) in query.iter(world) {
            if world.get::<Connected>(controlled.owner).is_none() {
                continue;
            }
            if riders.iter().any(|r: &Rider| r.link == controlled.owner) {
                ambiguous.insert(controlled.owner);
            }
            riders.push(Rider {
                link: controlled.owner,
                entity,
                alive: alive.0,
                generation: generation.0,
            });
        }
    }
    // Collect per-link queues without holding borrows across the write phase.
    let links: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, (With<Connected>, With<TurnQueue>)>();
        q.iter(world).collect()
    };
    for link in links {
        // Expire ~1s-old commands first; ack each as Expired.
        let expired: Vec<(u64, u128)> = world
            .get_mut::<TurnQueue>(link)
            .map(|queue| queue.into_inner().expire(now_tick))
            .unwrap_or_default();
        for (seq, generation) in expired {
            tracing::debug!(
                ?link,
                seq,
                generation,
                execution_tick = now_tick,
                "Turn command expired in queue"
            );
            ack_on(
                world,
                link,
                TurnAck {
                    seq,
                    outcome: TurnOutcome::Rejected {
                        reason: TurnRejectReason::Expired,
                    },
                },
            );
        }
        let rider = riders.iter().find(|r| r.link == link);
        let eligible = match rider {
            Some(r) if !ambiguous.contains(&link) && r.alive => Some((r.entity, r.generation)),
            _ => None,
        };
        let Some((rider_entity, current_generation)) = eligible else {
            // No eligible rider: reject the whole queue at execution time.
            // Dead rider → NotAlive; ambiguous/missing mapping → NotEligible.
            let pending: Vec<(u64, u128, bool)> = world
                .get_mut::<TurnQueue>(link)
                .map(|q| {
                    q.into_inner()
                        .queue
                        .drain(..)
                        .map(|c| (c.seq, c.generation, rider.is_some()))
                        .collect()
                })
                .unwrap_or_default();
            for (seq, generation, has_rider) in pending {
                let reason = if has_rider {
                    TurnRejectReason::NotAlive
                } else {
                    TurnRejectReason::NotEligible
                };
                tracing::debug!(
                    ?link,
                    seq,
                    generation,
                    ?reason,
                    execution_tick = now_tick,
                    "Turn command rejected: no eligible rider"
                );
                ack_on(
                    world,
                    link,
                    TurnAck {
                        seq,
                        outcome: TurnOutcome::Rejected { reason },
                    },
                );
            }
            continue;
        };
        // Stale-life queued commands are rejected at execution time.
        let stale: Vec<(u64, u128)> = world
            .get_mut::<TurnQueue>(link)
            .map(|q| {
                let queue = q.into_inner();
                let mut stale = Vec::new();
                queue.queue.retain(|c| {
                    if c.generation != current_generation {
                        stale.push((c.seq, c.generation));
                        false
                    } else {
                        true
                    }
                });
                stale
            })
            .unwrap_or_default();
        for (seq, generation) in stale {
            tracing::debug!(
                ?link,
                seq,
                generation,
                current_generation,
                execution_tick = now_tick,
                "Turn command rejected: stale life"
            );
            ack_on(
                world,
                link,
                TurnAck {
                    seq,
                    outcome: TurnOutcome::Rejected {
                        reason: TurnRejectReason::StaleLife,
                    },
                },
            );
        }
        // Pop at most one command for this tick.
        let next: Option<QueuedTurn> = world
            .get_mut::<TurnQueue>(link)
            .and_then(|q| q.into_inner().queue.pop_front());
        match next {
            Some(cmd) => {
                let input = cmd.direction.to_input(current_generation);
                if let Some(mut action) =
                    world.get_mut::<shared::ActionState<shared::PlayerInput>>(rider_entity)
                {
                    *action = shared::ActionState(input);
                }
                let depth = world
                    .get::<TurnQueue>(link)
                    .map(|q| q.queue.len())
                    .unwrap_or(0);
                tracing::debug!(
                    ?link,
                    rider = ?rider_entity,
                    seq = cmd.seq,
                    ?cmd.direction,
                    generation = cmd.generation,
                    client_tick = cmd.client_tick,
                    execution_tick = now_tick,
                    queue_depth = depth,
                    "Turn command applied"
                );
                ack_on(
                    world,
                    link,
                    TurnAck {
                        seq: cmd.seq,
                        outcome: TurnOutcome::Applied {
                            server_tick: now_tick,
                        },
                    },
                );
            }
            None => {
                // Missing input executes nothing: explicitly clear any
                // `get_predict` repeat so no phantom turn fires.
                if let Some(mut action) =
                    world.get_mut::<shared::ActionState<shared::PlayerInput>>(rider_entity)
                {
                    *action = shared::ActionState(shared::PlayerInput::None);
                }
            }
        }
    }
    // Bots and link-less humans are untouched above; belt-and-braces: any
    // Human rider whose link has no queue keeps whatever ActionState the input
    // pipeline left — but that pipeline only ever wrote None for humans
    // without commands this tick... except `get_predict` repeats. Rider
    // entities always have a link in practice (spawned in on_client_connected),
    // and queues are created on first command; to close the phantom-turn gap
    // for command-less links, clear ActionState on Human riders whose link has
    // no queue this tick.
    clear_unqueued_human_inputs(world);
}

/// Humans whose link has no `TurnQueue` yet never received a command; force
/// `None` so a stale native-input repeat can't fire a phantom turn.
fn clear_unqueued_human_inputs(world: &mut World) {
    let riders: Vec<(Entity, Entity)> = world
        .query_filtered::<(Entity, &ControlledBy), (With<Human>, With<shared::Player>)>()
        .iter(world)
        .map(|(e, c)| (e, c.owner))
        .collect();
    for (rider, link) in riders {
        if world.get::<TurnQueue>(link).is_some() {
            continue;
        }
        if world.get::<Connected>(link).is_none() {
            continue;
        }
        if let Some(mut action) = world.get_mut::<shared::ActionState<shared::PlayerInput>>(rider) {
            *action = shared::ActionState(shared::PlayerInput::None);
        }
    }
}

fn ack_on(world: &mut World, link: Entity, ack: TurnAck) {
    if let Some(mut sender) = world.get_mut::<MessageSender<TurnAck>>(link) {
        sender.send::<TurnChannel>(ack);
    }
}

/// Clear a rider's queued turns when it dies or respawns (generation bump
/// makes old ones stale anyway; explicit clear is preferred so nothing
/// lingers across lives). Tracks (alive, generation) per link; any change
/// that ends a life (death or new generation) drains that link's queue.
#[allow(clippy::type_complexity)]
pub fn clear_turn_queues_on_life_change(
    riders: Query<
        (&ControlledBy, &shared::IsAlive, &shared::LifeGeneration),
        (With<Human>, With<shared::Player>),
    >,
    mut queues: Query<&mut TurnQueue>,
    mut seen: Local<std::collections::HashMap<Entity, (bool, u128)>>,
) {
    use std::collections::HashSet;
    let mut live_links: HashSet<Entity> = HashSet::new();
    for (controlled, alive, generation) in &riders {
        let link = controlled.owner;
        live_links.insert(link);
        let current = (alive.0, generation.0);
        match seen.insert(link, current) {
            // Same life, still alive: keep queue.
            Some((true, previous)) if previous == current.1 && current.0 => {}
            // First sight of this link: nothing to clear.
            None => {}
            // Death or generation bump: drain the queue.
            _ => {
                if let Ok(mut queue) = queues.get_mut(link) {
                    queue.queue.clear();
                }
            }
        }
    }
    // Links with no rider anymore (despawned/disconnected) drop tracking;
    // their `TurnQueue` component dies with the link entity itself.
    seen.retain(|link, _| live_links.contains(link));
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
    // (Split insert: the 15-component tuple Bundle limit is already reached.)
    commands
        .spawn((
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
        ))
        .insert(shared::DisplayName("Player".to_string()));
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

    // --- One-shot turn delivery (phantom-turn fix) ---

    use shared::protocol::{TURN_QUEUE_CAP, TURN_QUEUE_TTL_TICKS, TurnDirection};

    fn setup_turns() -> (World, Entity, Entity) {
        let mut world = World::new();
        let (link, rider) = spawn_turn_link(&mut world);
        (world, link, rider)
    }

    fn spawn_turn_link(world: &mut World) -> (Entity, Entity) {
        let link = world
            .spawn((RemoteId(PeerId::Local(1)), Connected, TurnQueue::default()))
            .id();
        let rider = world
            .spawn((
                Player,
                Human,
                LifeGeneration(7),
                IsAlive(true),
                shared::ActionState(PlayerInput::None),
                ControlledBy {
                    owner: link,
                    lifetime: Lifetime::SessionBased,
                },
            ))
            .id();
        (link, rider)
    }

    fn cmd(seq: u64, direction: TurnDirection) -> TurnCommand {
        TurnCommand {
            seq,
            direction,
            generation: 7,
            client_tick: 100 + seq as u32,
        }
    }

    fn action(world: &World, rider: Entity) -> PlayerInput {
        world
            .get::<shared::ActionState<PlayerInput>>(rider)
            .unwrap()
            .0
    }

    #[test]
    fn turns_execute_in_seq_order_one_per_tick_including_identical() {
        let (mut world, link, rider) = setup_turns();
        // `collect_turn_commands` sorts each batch by (generation, seq) before
        // offering, so `offer` sees non-decreasing seqs and the accepted-seq
        // watermark doubles as the duplicate guard. Each press is its own
        // command even when the direction repeats.
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert!(queue.offer(cmd(0, TurnDirection::Left), 0).is_none());
        assert!(queue.offer(cmd(1, TurnDirection::Left), 0).is_none());
        assert!(queue.offer(cmd(2, TurnDirection::Right), 0).is_none());

        // One command per fixed tick, in seq order.
        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::TurnLeftFor(7));
        assert_eq!(world.get::<TurnQueue>(link).unwrap().queue.len(), 2);
        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::TurnLeftFor(7));
        assert_eq!(world.get::<TurnQueue>(link).unwrap().queue.len(), 1);
        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::TurnRightFor(7));
        assert!(world.get::<TurnQueue>(link).unwrap().queue.is_empty());
    }

    #[test]
    fn duplicate_commands_execute_exactly_once() {
        let (mut world, link, rider) = setup_turns();
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert!(queue.offer(cmd(0, TurnDirection::Left), 0).is_none());

        // Resent press (same generation + seq) is a duplicate, never queued.
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert_eq!(
            queue.offer(cmd(0, TurnDirection::Left), 0),
            Some(TurnOutcome::Rejected {
                reason: TurnRejectReason::Duplicate
            })
        );

        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::TurnLeftFor(7));
        // Late duplicate of an already-executed seq is still a duplicate.
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert_eq!(
            queue.offer(cmd(0, TurnDirection::Left), 0),
            Some(TurnOutcome::Rejected {
                reason: TurnRejectReason::Duplicate
            })
        );

        // Nothing left to execute: missing input yields no turn, not a repeat.
        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::None);
    }

    #[test]
    fn stale_life_commands_are_rejected() {
        let (mut world, link, rider) = setup_turns();
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert!(
            queue
                .offer(
                    TurnCommand {
                        generation: 6,
                        ..cmd(0, TurnDirection::Left)
                    },
                    0
                )
                .is_none()
        );

        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::None);
        assert!(world.get::<TurnQueue>(link).unwrap().queue.is_empty());
    }

    #[test]
    fn dead_rider_rejects_queued_turns() {
        let (mut world, link, rider) = setup_turns();
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert!(queue.offer(cmd(0, TurnDirection::Right), 0).is_none());

        world.get_mut::<IsAlive>(rider).unwrap().0 = false;
        apply_turn_commands(&mut world);
        assert!(world.get::<TurnQueue>(link).unwrap().queue.is_empty());
        // No eligible rider, so no turn is written for the corpse.
        assert_eq!(action(&world, rider), PlayerInput::None);
    }

    #[test]
    fn missing_input_writes_none_never_a_repeat() {
        let (mut world, _, rider) = setup_turns();
        // Simulate Lightyear's `get_predict` fallback repeating a stale turn.
        world
            .get_mut::<shared::ActionState<PlayerInput>>(rider)
            .unwrap()
            .0 = PlayerInput::TurnLeftFor(7);
        apply_turn_commands(&mut world);
        assert_eq!(action(&world, rider), PlayerInput::None);
    }

    #[test]
    fn queue_full_rejects_new_without_dropping_accepted() {
        let (mut world, link, _) = setup_turns();
        for seq in 0..TURN_QUEUE_CAP as u64 {
            let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
            assert!(queue.offer(cmd(seq, TurnDirection::Left), 0).is_none());
        }
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert_eq!(
            queue.offer(cmd(TURN_QUEUE_CAP as u64, TurnDirection::Left), 0),
            Some(TurnOutcome::Rejected {
                reason: TurnRejectReason::QueueFull
            })
        );

        // Accepted commands are never silently dropped to make room.
        assert_eq!(
            world.get::<TurnQueue>(link).unwrap().queue.len(),
            TURN_QUEUE_CAP
        );
    }

    #[test]
    fn very_old_queued_entries_expire() {
        let (mut world, link, _) = setup_turns();
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert!(queue.offer(cmd(0, TurnDirection::Left), 10).is_none());
        let expired = queue.expire(10 + TURN_QUEUE_TTL_TICKS + 1);
        assert_eq!(expired, vec![(0, 7)]);
        assert!(queue.queue.is_empty());
    }

    #[test]
    fn disconnect_drains_queue_and_removes_turn_state_with_the_link() {
        let (mut world, link, rider) = setup_turns();
        let mut queue = world.get_mut::<TurnQueue>(link).unwrap();
        assert!(queue.offer(cmd(0, TurnDirection::Left), 0).is_none());

        // Link gone (no `Connected`): `apply_turn_commands` only iterates
        // connected links, so the stale queue is inert — never executed.
        world.entity_mut(link).remove::<Connected>();
        apply_turn_commands(&mut world);
        assert_eq!(world.get::<TurnQueue>(link).unwrap().queue.len(), 1);
        assert_eq!(action(&world, rider), PlayerInput::None);
        // Full despawn takes the queue component with it: nothing outlives the
        // link, so no turn state can leak into a later connection.
        world.despawn(link);
        world.despawn(rider);
        apply_turn_commands(&mut world);
        assert!(
            world
                .query_filtered::<Entity, With<TurnQueue>>()
                .iter(&world)
                .count()
                == 0
        );
    }

    #[test]
    fn bots_keep_direct_action_state_writes() {
        let (mut world, _, _) = setup_turns();
        // A bot rider: `Player` without `Human`/`ControlledBy`, holding a
        // directly-written turn from `bot_brain_input`.
        let bot = world
            .spawn((
                Player,
                IsAlive(true),
                shared::ActionState(PlayerInput::TurnLeft),
            ))
            .id();
        apply_turn_commands(&mut world);
        assert_eq!(action(&world, bot), PlayerInput::TurnLeft);
    }

    #[test]
    fn life_change_clears_queue_on_death_and_respawn() {
        // The tracker lives in a `Local`, so drive the real system through an
        // `App` — `run_system_once` would reset the baseline every call.
        let mut app = App::new();
        let (link, rider) = spawn_turn_link(app.world_mut());
        app.add_systems(Update, clear_turn_queues_on_life_change);
        app.update(); // Baseline: alive in life 7.
        app.world_mut()
            .get_mut::<TurnQueue>(link)
            .unwrap()
            .offer(cmd(0, TurnDirection::Left), 0);
        // Death clears the queue without executing it.
        app.world_mut().get_mut::<IsAlive>(rider).unwrap().0 = false;
        app.update();
        assert!(app.world().get::<TurnQueue>(link).unwrap().queue.is_empty());
        // A command queued while dead is cleared on respawn into a new life.
        app.world_mut()
            .get_mut::<TurnQueue>(link)
            .unwrap()
            .offer(cmd(1, TurnDirection::Right), 0);
        app.world_mut()
            .entity_mut(rider)
            .insert((IsAlive(true), LifeGeneration(8)));
        app.update();
        assert!(app.world().get::<TurnQueue>(link).unwrap().queue.is_empty());
        apply_turn_commands(app.world_mut());
        assert_eq!(action(app.world(), rider), PlayerInput::None);
    }
}
