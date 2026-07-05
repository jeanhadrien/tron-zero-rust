//! Lightyear protocol registration — called by both client and server before
//! spawning the link entity.
//!
//! Registers all replicated components, prediction targets, and the input type
//! so the protocol checksums match across peers.

use crate::*;
use bevy_app::App;
use lightyear::prelude::input::native::InputPlugin;
use lightyear::prelude::{AppComponentExt, PredictionBuilderExt};

pub fn register_protocol(app: &mut App) {
    // --- Input ---
    app.add_plugins(InputPlugin::<PlayerInput>::default());

    // --- Predicted components (client predicts, server corrects) ---
    app.component::<Position>().replicate().predict();
    app.component::<Direction>().replicate().predict();
    app.component::<Velocity>().replicate().predict();
    app.component::<SpeedMult>().replicate().predict();
    app.component::<TargetSpeedMult>().replicate().predict();
    app.component::<Rubber>().replicate().predict();
    app.component::<IsAlive>().replicate().predict();
    app.component::<IsSliding>().replicate().predict();
    app.component::<IsColliding>().replicate().predict();
    app.component::<ShouldHandleDeath>().replicate().predict();
    app.component::<TrailPointCount>().replicate().predict();
    app.component::<TrailPointNextOrder>().replicate().predict();

    // --- Replicated-only (no prediction needed) ---
    app.component::<Player>().replicate_once();
    app.component::<PlayerId>().replicate_once();
    app.component::<PlayerColor>().replicate_once();

    app.component::<TrailPoint>().replicate();
    app.component::<TrailPointOrder>().replicate();

    app.component::<Arena>().replicate_once();
    app.component::<ArenaSize>().replicate_once();
    app.component::<WallSegments>().replicate_once();

    // ActionState is local per-tick state (written by input systems, read by
    // apply_turn). Not replicated — the lightyear input system handles
    // transmitting the value. Both sides add it manually on their entity.
}
