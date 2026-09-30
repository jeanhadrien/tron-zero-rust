//! Lightyear protocol registration — called by both client and server before
//! spawning the link entity.
//!
//! Registers all replicated components, prediction targets, and the input type
//! so the protocol checksums match across peers.

use crate::*;
use bevy_app::App;
use lightyear::prelude::input::native::InputPlugin;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RespawnRequest {
    pub generation: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RespawnOutcome {
    Accepted,
    NotEligible,
    NoSafeSpace,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RespawnReply {
    pub generation: u128,
    pub outcome: RespawnOutcome,
}

pub struct RespawnChannel;

pub fn register_protocol(app: &mut App) {
    app.add_channel::<RespawnChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..Default::default()
    })
    .add_direction(NetworkDirection::Bidirectional);
    app.register_message::<RespawnRequest>()
        .add_direction(NetworkDirection::ClientToServer);
    app.register_message::<RespawnReply>()
        .add_direction(NetworkDirection::ServerToClient);
    // --- Input ---
    app.add_plugins(InputPlugin::<PlayerInput>::default());

    // --- Predicted components (client predicts, server corrects) ---
    app.component::<Position>()
        .replicate()
        .predict()
        .add_interpolation_with(step);
    app.component::<Direction>()
        .replicate()
        .predict()
        .add_interpolation_with(step);
    app.component::<Velocity>().replicate().predict();
    app.component::<SpeedMult>().replicate().predict();
    app.component::<TargetSpeedMult>().replicate().predict();
    app.component::<Rubber>().replicate().predict();
    app.component::<IsAlive>()
        .replicate()
        .predict()
        .add_interpolation_with(step);
    app.component::<IsSliding>().replicate().predict();
    app.component::<IsColliding>().replicate().predict();
    app.component::<ShouldHandleDeath>().replicate().predict();
    app.component::<LifeGeneration>().replicate().predict();
    app.component::<Trail>()
        .replicate()
        .predict()
        .add_interpolation_with(step);

    // --- Replicated-only (no prediction needed) ---
    app.component::<Player>().replicate_once();
    app.component::<PlayerId>().replicate_once();
    app.component::<PlayerColor>().replicate_once();

    app.component::<Arena>().replicate_once();
    app.component::<ArenaSize>().replicate_once();
    app.component::<WallSegments>().replicate_once();

    // ActionState is local per-tick state (written by input systems, read by
    // simulate_players). Not replicated — the lightyear input system handles
    // transmitting the value. Both sides add it manually on their entity.
}

// Discrete snapshots keep cardinal turns and trail heads on the same tick.
fn step<T>(start: T, end: T, t: f32) -> T {
    if t >= 1.0 { end } else { start }
}
