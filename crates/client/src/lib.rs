//! Tron Zero — client library.
//!
//! Everything the native client is made of, so integration tests can drive the
//! real connection, input and prediction systems without a window
//! (`tron-zero-nettest`). `main.rs` only adds windowing, menus and rendering.

pub mod browser;
pub mod config;
pub mod connection;
pub mod input;
pub mod menu;
pub mod render;
pub mod settings;
pub mod sim;
pub mod theme;

use bevy::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;

/// Lightyear client plugins at the shared tick rate, plus the protocol
/// registration that must match the server's.
pub fn add_network(app: &mut App) {
    app.add_plugins(ClientPlugins {
        tick_duration: core::time::Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });
    shared::protocol::register_protocol(app);
}

/// Keyboard → turn commands and local prediction: input resources, the
/// receive/buffer systems, the fixed-tick writer and the shared simulation.
pub fn add_input_and_prediction(app: &mut App) {
    app.init_resource::<input::PendingInput>();
    app.init_resource::<input::InputLifecycle>();
    app.init_resource::<input::TurnSeq>();
    app.init_resource::<input::TurnAckState>();
    app.init_resource::<input::TurnOutbox>();
    app.init_resource::<input::RespawnUi>();
    app.add_systems(
        PreUpdate,
        (
            input::receive_respawn_replies,
            input::receive_turn_acks,
            input::buffer_keyboard_input,
        )
            .chain()
            .after(bevy::input::InputSystems)
            .after(MessageSystems::Receive),
    );
    // The outbox leaves in this frame's packet: after the fixed ticks that
    // filled it, before Lightyear serializes the messages.
    app.add_systems(
        PostUpdate,
        input::send_turn_batch.before(MessageSystems::Send),
    );
    app.add_observer(input::follow_timeline_resync);
    app.add_systems(
        FixedPreUpdate,
        input::read_keyboard
            .in_set(lightyear::prelude::client::input::InputSystems::WriteClientInputs),
    );

    // Simulation: same systems as the server, running on the predicted entity.
    // Runs in FixedUpdate, which is strictly after FixedPreUpdate (where
    // lightyear's WriteClientInputs → BufferClientInputs chains), so ActionState
    // is already populated for this tick.
    app.insert_resource(shared::SimulationRole::Client);
    app.add_systems(FixedUpdate, shared::simulate_players);
}

/// Reconcile after replication, regardless of Player/Controlled arrival order.
#[allow(clippy::type_complexity)]
pub fn attach_local_input(
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
