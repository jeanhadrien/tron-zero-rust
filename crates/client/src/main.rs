//! Tron Zero — client entry point.
//!
//! Connects to a lightyear server via UDP / raw connection, sends keyboard
//! inputs, predicts the local lightcycle, and renders the arena + players.

mod connection;
mod input;
mod menu;
mod render;
mod settings;
mod theme;

use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use core::time::Duration;
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::client::*;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins);
    app.add_plugins(EguiPlugin::default());

    // --- Lightyear ---
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });

    // Register components + input type so protocol checksums match the server.
    shared::protocol::register_protocol(&mut app);

    app.init_resource::<connection::Session>();
    app.init_resource::<menu::MenuState>();
    app.init_resource::<settings::TurnBindings>();
    app.init_resource::<settings::RebindState>();
    app.add_systems(
        PreUpdate,
        (
            menu::begin_input_frame,
            attach_local_input,
            connection::monitor_connection,
            menu::menu_controls,
        )
            .chain()
            .after(bevy::input::InputSystems)
            .after(bevy::ui::UiSystems::Focus)
            .after(MessageSystems::Receive)
            .after(ReplicationSystems::Receive)
            .before(input::buffer_keyboard_input),
    );
    app.add_systems(Update, connection::finish_disconnect);
    // egui overlays share one pass; order keeps the menu above HUD/death on
    // any same-order tie (menu.open also hides both outright).
    app.add_systems(
        EguiPrimaryContextPass,
        (render::hud_ui, render::death_ui, menu::menu_ui),
    );
    app.add_systems(Update, connection::send_leave);
    app.add_systems(
        PostUpdate,
        connection::unlink_session.after(LinkSystems::Send),
    );

    // Input: buffer key presses every frame, consume per fixed tick.
    app.init_resource::<input::PendingInput>();
    app.init_resource::<input::InputLifecycle>();
    app.init_resource::<input::RespawnUi>();
    app.add_systems(
        PreUpdate,
        (input::receive_respawn_replies, input::buffer_keyboard_input)
            .chain()
            .after(bevy::input::InputSystems)
            .after(MessageSystems::Receive),
    );
    app.add_systems(
        FixedPreUpdate,
        input::read_keyboard.in_set(InputSystems::WriteClientInputs),
    );

    // Simulation: same systems as the server, running on the predicted entity.
    // Runs in FixedUpdate, which is strictly after FixedPreUpdate (where
    // lightyear's WriteClientInputs → BufferClientInputs chains), so ActionState
    // is already populated for this tick.
    app.insert_resource(shared::SimulationRole::Client);
    app.add_systems(FixedUpdate, shared::simulate_players);

    // Rendering.
    app.add_systems(Startup, render::setup_camera);
    app.add_systems(
        Update,
        (
            render::draw_arena,
            render::draw_trails,
            render::draw_players,
            render::follow_player,
        ),
    );

    app.run();
}

/// Reconcile after replication, regardless of Player/Controlled arrival order.
#[allow(clippy::type_complexity)]
fn attach_local_input(
    mut commands: Commands,
    players: Query<
        (Entity, Option<&ControlledBy>),
        (
            With<shared::Player>,
            With<Controlled>,
            Without<InputMarker<shared::PlayerInput>>,
        ),
    >,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
) {
    let Ok(client) = clients.single() else {
        return;
    };
    for (entity, owner) in &players {
        if owner.is_none_or(|owner| owner.owner == client) {
            commands
                .entity(entity)
                .insert(InputMarker::<shared::PlayerInput>::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn local_input_setup_handles_both_replication_orders() {
        for control_first in [true, false] {
            let mut world = World::new();
            world.spawn((Client::default(), RemoteId(PeerId::Server), Connected));
            let player = world.spawn_empty().id();
            if control_first {
                world.entity_mut(player).insert(Controlled);
            } else {
                world.entity_mut(player).insert(shared::Player);
            }
            world.run_system_once(attach_local_input).unwrap();
            assert!(
                world
                    .get::<InputMarker<shared::PlayerInput>>(player)
                    .is_none()
            );
            if control_first {
                world.entity_mut(player).insert(shared::Player);
            } else {
                world.entity_mut(player).insert(Controlled);
            }
            world.run_system_once(attach_local_input).unwrap();
            assert!(
                world
                    .get::<InputMarker<shared::PlayerInput>>(player)
                    .is_some()
            );
            assert!(world.get::<ControlledBy>(player).is_none());
        }
    }
}
