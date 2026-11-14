//! Headless server for network tests: the production server wiring
//! (`tron_zero_server::add_core`), no bots, plus tracing.

use std::collections::BTreeMap;

use bevy::prelude::*;
use glam::Vec2;
use lightyear::prelude::input::native::NativeBuffer;
use lightyear::prelude::*;
use shared::{ActionState, Direction, IsAlive, PlayerId, PlayerInput, Position, Trail};
use tron_zero_server as ts;
use ts::systems::Human;
use ts::turns::TurnLog;

use crate::{Impairment, install_conditioner};

/// What the server knew about one rider at the end of one fixed tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServerTick {
    pub position: Vec2,
    pub direction: Vec2,
    pub alive: bool,
    /// The `ActionState` `simulate_players` consumed: the executed turn.
    pub executed: PlayerInput,
    /// What the native Lightyear input buffer held for this tick. The server
    /// ignores it for humans; recorded as an oracle for the shared timeline.
    pub native: Option<PlayerInput>,
}

/// Per-`(player id, tick)` history of the authoritative simulation.
#[derive(Resource, Default)]
pub struct ServerTrace(pub BTreeMap<(String, u32), ServerTick>);

pub struct TestServer {
    pub app: App,
}

impl TestServer {
    pub fn new(port: u16, impairment: Impairment) -> Self {
        Self::build(port, None, None, impairment)
    }

    /// A server in rounds mode (what `mode = "rounds"` selects in production).
    pub fn with_rounds(port: u16, rules: shared::MatchRules, impairment: Impairment) -> Self {
        Self::build(port, None, Some(rules), impairment)
    }

    /// A server that also accepts WebTransport sessions on `web_port`, with a
    /// fresh self-signed certificate (its digest is `endpoint.cert_hash`).
    pub fn with_web(port: u16, web: ts::web::WebEndpoint, impairment: Impairment) -> Self {
        Self::build(port, Some(web), None, impairment)
    }

    fn build(
        port: u16,
        web: Option<ts::web::WebEndpoint>,
        rules: Option<shared::MatchRules>,
        impairment: Impairment,
    ) -> Self {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        if let Some(rules) = rules {
            app.insert_resource(ts::rounds::MatchState::new(rules));
        }
        ts::add_core(&mut app, port);
        if let Some(web) = web {
            ts::add_web(&mut app, web);
        }
        app.init_resource::<TurnLog>();
        app.init_resource::<ServerTrace>();
        app.add_systems(FixedLast, record_server_tick);
        app.init_resource::<SentBytes>();
        // After the transport has written this frame's packets into each link,
        // before the IO backend drains them.
        app.add_systems(
            PostUpdate,
            count_sent_bytes
                .after(lightyear_transport::plugin::TransportSystems::Send)
                .before(LinkSystems::Send),
        );
        install_conditioner(&mut app, impairment);
        // Startup systems (arena + listener) run on the first update.
        app.finish();
        app.cleanup();
        app.update();
        Self { app }
    }

    /// The server's last simulated tick.
    pub fn tick(&self) -> u32 {
        self.app.world().resource::<LocalTimeline>().tick().0
    }

    /// Payload bytes the server has handed to client links so far, summed over
    /// every link (packet payloads, without UDP/IP headers).
    pub fn sent_bytes(&self) -> u64 {
        self.app.world().resource::<SentBytes>().0
    }

    pub fn turn_log(&self) -> &TurnLog {
        self.app.world().resource::<TurnLog>()
    }

    pub fn trace(&self) -> &ServerTrace {
        self.app.world().resource::<ServerTrace>()
    }

    /// Ids of the connected human riders.
    pub fn player_ids(&mut self) -> Vec<String> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&PlayerId, With<Human>>();
        query.iter(world).map(|id| id.0.clone()).collect()
    }

    /// Cap the number of human riders (what `server.max_players` does in production).
    pub fn set_max_players(&mut self, max: u32) {
        self.app
            .world_mut()
            .insert_resource(ts::admission::MaxPlayers(max));
    }

    /// Number of client links the server currently holds (admitted or not).
    pub fn link_count(&mut self) -> usize {
        let world = self.app.world_mut();
        world
            .query_filtered::<(), With<lightyear::prelude::server::ClientOf>>()
            .iter(world)
            .count()
    }

    /// Number of client links the server replicates the world to.
    pub fn replicating_link_count(&mut self) -> usize {
        let world = self.app.world_mut();
        world
            .query_filtered::<(), (
                With<lightyear::prelude::server::ClientOf>,
                With<ReplicationSender>,
            )>()
            .iter(world)
            .count()
    }

    /// Display names of the human riders, sorted.
    pub fn human_names(&mut self) -> Vec<String> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<&shared::DisplayName, With<Human>>();
        let mut names: Vec<String> = query.iter(world).map(|name| name.0.clone()).collect();
        names.sort();
        names
    }

    /// Player id of the human rider displayed as `name`.
    pub fn player_id_named(&mut self, name: &str) -> Option<String> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<(&PlayerId, &shared::DisplayName), With<Human>>();
        query
            .iter(world)
            .find(|(_, display)| display.0 == name)
            .map(|(id, _)| id.0.clone())
    }

    /// Color and shape of the human rider displayed as `name`.
    pub fn look_of(&mut self, name: &str) -> Option<(u32, shared::RiderShape)> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<(
            &shared::DisplayName,
            &shared::PlayerColor,
            &shared::RiderShape,
        ), With<Human>>();
        query
            .iter(world)
            .find(|(display, ..)| display.0 == name)
            .map(|(_, color, shape)| (color.0, *shape))
    }

    /// Spawn a brainless rider with an explicit look. It is replicated and
    /// interpolated like a bot but is only a reduced stand-in for
    /// `spawn_bot_entity`'s bundle (no bot components).
    pub fn spawn_bot_rider(&mut self, name: &str, color: u32, shape: shared::RiderShape) {
        self.app.world_mut().spawn((
            shared::Player,
            shared::PlayerId("bot-test".into()),
            shared::DisplayName(name.into()),
            shared::PlayerColor(color),
            shape,
            Position(Vec2::ZERO),
            Direction(Vec2::X),
            IsAlive(true),
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
        ));
    }

    /// Rider entity and its owning link, by player id.
    pub fn rider(&mut self, player_id: &str) -> Option<(Entity, Entity)> {
        let world = self.app.world_mut();
        let mut query = world.query_filtered::<(Entity, &PlayerId, &ControlledBy), With<Human>>();
        query
            .iter(world)
            .find(|(_, id, _)| id.0 == player_id)
            .map(|(rider, _, controlled)| (rider, controlled.owner))
    }

    /// Put a rider at a known pose with a fresh trail, so scripted turns stay
    /// inside the arena. Replication hands the pose to the client, which
    /// corrects itself once; tests settle before they start measuring.
    pub fn place_rider(&mut self, player_id: &str, position: Vec2, direction: Vec2) {
        let (rider, _) = self.rider(player_id).expect("rider exists");
        self.app.world_mut().entity_mut(rider).insert((
            Position(position),
            Direction(direction),
            Trail::new(position),
        ));
    }

    /// The authoritative `Trail` corners of a rider, by player id.
    pub fn trail_of(&mut self, player_id: &str) -> Option<Trail> {
        let (rider, _) = self.rider(player_id)?;
        self.app.world().get::<Trail>(rider).cloned()
    }

    /// Kill a rider as if it had crashed (the queue and life rules must cope).
    pub fn kill_rider(&mut self, player_id: &str) {
        let (rider, _) = self.rider(player_id).expect("rider exists");
        self.app
            .world_mut()
            .entity_mut(rider)
            .insert(IsAlive(false));
    }
}

/// Running total behind [`TestServer::sent_bytes`].
#[derive(Resource, Default)]
struct SentBytes(u64);

/// Adds up the payloads queued on every client link this frame. The queue is
/// drained and refilled in order, since `LinkSender` only iterates by draining.
fn count_sent_bytes(
    mut links: Query<&mut Link, With<lightyear::prelude::server::ClientOf>>,
    mut total: ResMut<SentBytes>,
    mut scratch: Local<Vec<lightyear::link::SendPayload>>,
) {
    for mut link in &mut links {
        scratch.extend(link.send.drain());
        for payload in scratch.drain(..) {
            total.0 += payload.len() as u64;
            link.send.push(payload);
        }
    }
}

fn record_server_tick(
    timeline: Res<LocalTimeline>,
    riders: Query<
        (
            &PlayerId,
            &Position,
            &Direction,
            &IsAlive,
            &ActionState<PlayerInput>,
            Option<&NativeBuffer<PlayerInput>>,
        ),
        With<Human>,
    >,
    mut trace: ResMut<ServerTrace>,
) {
    let tick = timeline.tick();
    for (id, position, direction, alive, action, native) in &riders {
        trace.0.insert(
            (id.0.clone(), tick.0),
            ServerTick {
                position: position.0,
                direction: direction.0,
                alive: alive.0,
                executed: action.0,
                native: native.and_then(|buffer| buffer.get(tick)).map(|a| a.0),
            },
        );
    }
}
