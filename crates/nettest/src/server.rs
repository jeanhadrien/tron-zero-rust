//! Headless server for network tests: the production server wiring
//! (`tron_zero_server::add_core`), no bots, plus tracing.

use std::collections::BTreeMap;

use bevy::prelude::*;
use glam::Vec2;
use lightyear::prelude::input::native::NativeBuffer;
use lightyear::prelude::*;
use shared::{ActionState, Direction, IsAlive, PlayerId, PlayerInput, Position, Trail, Velocity};
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
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        ts::add_core(&mut app, port);
        app.init_resource::<TurnLog>();
        app.init_resource::<ServerTrace>();
        app.add_systems(FixedLast, record_server_tick);
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
            Velocity(direction * (shared::BASE_SPEED * shared::TICK_SECS * 1000.0)),
            Trail::new(position),
        ));
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

#[allow(clippy::type_complexity)]
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
