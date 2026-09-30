//! One persistent screen-space prompt, independent of rider/camera lifetimes.

use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, ControlledBy, Predicted};
use shared::protocol::RespawnOutcome;
use shared::{IsAlive, Player, PlayerInput};

use crate::input::RespawnUi;

#[derive(Component)]
pub struct DeathOverlay;

#[derive(Component)]
pub struct DeathMessage;

pub fn setup_death_overlay(mut commands: Commands) {
    commands
        .spawn((
            DeathOverlay,
            GlobalZIndex(20),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.0),
                height: percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                display: Display::None,
                ..Default::default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: px(440.0),
                    max_width: percent(90.0),
                    padding: UiRect::all(px(28.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: px(16.0),
                    border: UiRect::top(px(2.0)),
                    ..Default::default()
                },
                BorderColor::all(Color::srgb(1.0, 0.3, 0.3)),
                BackgroundColor(Color::srgba(0.03, 0.06, 0.09, 0.96)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("YOU DIED"),
                    TextFont {
                        font_size: FontSize::Px(36.0),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(1.0, 0.3, 0.3)),
                ));
                panel.spawn((
                    DeathMessage,
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(18.0),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.75, 0.82, 0.88)),
                    TextLayout::justify(Justify::Center),
                ));
            });
        });
}

fn prompt(alive: bool, respawn: &RespawnUi) -> &'static str {
    if alive {
        ""
    } else if respawn.pending_generation.is_some() {
        "Respawn requested — waiting for server..."
    } else {
        match respawn.outcome {
            Some(RespawnOutcome::NoSafeSpace) => {
                "No safe spawn space. Press Space / Enter to retry."
            }
            Some(RespawnOutcome::NotEligible) => {
                "Server has not confirmed eligibility. Press Space / Enter to retry."
            }
            _ => "Press Space / Enter to respawn",
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn update_death_overlay(
    session: Res<crate::connection::Session>,
    menu: Res<crate::menu::MenuState>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    players: Query<
        (&IsAlive, Option<&ControlledBy>),
        (
            With<Player>,
            With<Predicted>,
            With<InputMarker<PlayerInput>>,
        ),
    >,
    respawn: Res<RespawnUi>,
    mut overlay: Query<&mut Node, With<DeathOverlay>>,
    mut messages: Query<&mut Text, With<DeathMessage>>,
) {
    let local = clients.iter().next().and_then(|client| {
        players
            .iter()
            .find(|(_, controlled)| controlled.is_none_or(|c| c.owner == client))
    });
    let text = if menu.open || session.phase != crate::connection::ConnectionPhase::Playing {
        ""
    } else {
        local.map_or("", |(alive, _)| prompt(alive.0, &respawn))
    };
    for mut label in &mut messages {
        if label.0 != text {
            label.0 = text.to_owned();
        }
    }
    for mut node in &mut overlay {
        node.display = if text.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{ConnectionPhase, Session};
    use crate::menu::MenuState;
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::{Controlled, PeerId, RemoteId};

    #[test]
    fn death_panel_tracks_life_menu_and_disconnect() {
        let mut world = World::new();
        world.init_resource::<Session>();
        world.init_resource::<MenuState>();
        world.init_resource::<RespawnUi>();
        world.run_system_once(setup_death_overlay).unwrap();
        world.spawn((Client::default(), RemoteId(PeerId::Server), Connected));
        let rider = world
            .spawn((
                Player,
                Controlled,
                Predicted,
                InputMarker::<PlayerInput>::default(),
                IsAlive(false),
            ))
            .id();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.resource_mut::<MenuState>().open = false;
        world.run_system_once(update_death_overlay).unwrap();
        let root = world
            .query_filtered::<Entity, With<DeathOverlay>>()
            .single(&world)
            .unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::Flex);
        world.resource_mut::<RespawnUi>().pending_generation = Some(1);
        world.run_system_once(update_death_overlay).unwrap();
        let text = world
            .query_filtered::<&Text, With<DeathMessage>>()
            .single(&world)
            .unwrap();
        assert!(text.0.contains("waiting for server"));
        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(update_death_overlay).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
        world.resource_mut::<MenuState>().open = false;
        world.entity_mut(rider).insert(IsAlive(true));
        world.run_system_once(update_death_overlay).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
        world.entity_mut(rider).insert(IsAlive(false));
        world.resource_mut::<Session>().phase = ConnectionPhase::Disconnecting;
        world.run_system_once(update_death_overlay).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
    }

    #[test]
    fn prompt_tracks_dead_pending_rejected_and_alive_states() {
        let mut ui = RespawnUi::default();
        assert!(prompt(false, &ui).contains("Space / Enter"));
        ui.pending_generation = Some(1);
        assert!(prompt(false, &ui).contains("waiting for server"));
        assert_eq!(prompt(true, &ui), "");
        ui.pending_generation = None;
        ui.outcome = Some(RespawnOutcome::NoSafeSpace);
        assert!(prompt(false, &ui).contains("No safe spawn"));
        ui.outcome = Some(RespawnOutcome::NotEligible);
        assert!(prompt(false, &ui).contains("eligibility"));
    }
}
