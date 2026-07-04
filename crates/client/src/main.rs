//! Tron Zero — client entry point.
//!
//! MVP: a single controllable lightcycle on an empty arena.
//! - Renderer: gizmo-based arena walls + trail + cycle.
//! - Input: keyboard → turn left/right.
//! - Simulation: shared 120 Hz fixed systems (turn + move).

mod camera;
mod input;
mod render;

use bevy::prelude::*;
// `shared` (crate `tron-zero-shared`, renamed in Cargo.toml) is in the extern prelude.

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        // Pending turn input slot, drained by the fixed turn system.
        .init_resource::<shared::PendingInput>()
        // 120 Hz fixed simulation, matching the shared tick rate.
        .insert_resource(Time::<Fixed>::from_hz(shared::TICK_HZ))
        // World setup.
        .add_systems(Startup, (shared::setup_arena, shared::setup_local_player))
        .add_systems(Startup, render::setup_camera)
        // Per-frame: read keyboard, render, follow camera.
        .add_systems(
            Update,
            (input::read_keyboard, render::draw_arena, render::draw_trails, render::draw_players, camera::follow_player),
        )
        // Fixed: turn (Phase 1) then move (Phase 3), in order.
        .add_systems(FixedUpdate, (shared::apply_turn, shared::move_players).chain())
        .run();
}
