//! Persistent screen-space meters driven by the local predicted rider.

use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, Controlled, ControlledBy, Predicted};
use shared::{BASE_RUBBER, IsAlive, Player, PlayerInput, Rubber, SpeedMult};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;

#[derive(Component)]
pub struct HudRoot;

#[derive(Clone, Copy)]
enum Meter {
    Rubber,
    Speed,
}

#[derive(Component)]
pub struct MeterValue {
    meter: Meter,
    displayed: Option<u32>,
}

#[derive(Component)]
pub struct RubberFill;

const CYAN: Color = Color::srgb(0.0, 1.0, 0.8);

fn rubber_color(fraction: f32) -> Color {
    if fraction <= 0.2 {
        Color::srgb(1.0, 0.3, 0.3)
    } else if fraction <= 0.5 {
        Color::srgb(1.0, 0.75, 0.25)
    } else {
        CYAN
    }
}

pub fn setup_hud(mut commands: Commands) {
    commands
        .spawn((
            HudRoot,
            GlobalZIndex(10),
            Node {
                position_type: PositionType::Absolute,
                bottom: px(0.0),
                width: percent(100.0),
                padding: UiRect::all(px(20.0)),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::End,
                display: Display::None,
                ..Default::default()
            },
        ))
        .with_children(|root| {
            for (meter, label) in [(Meter::Rubber, "RUBBER"), (Meter::Speed, "SPEED")] {
                root.spawn((
                    Node {
                        width: percent(44.0),
                        max_width: px(220.0),
                        padding: UiRect::all(px(16.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(8.0),
                        ..Default::default()
                    },
                    BackgroundColor(Color::srgba(0.03, 0.06, 0.09, 0.9)),
                ))
                .with_children(|card| {
                    card.spawn((
                        Text::new(label),
                        TextFont {
                            font_size: FontSize::Px(14.0),
                            ..Default::default()
                        },
                        TextColor(Color::srgb(0.75, 0.82, 0.88)),
                    ));
                    card.spawn((
                        MeterValue {
                            meter,
                            displayed: None,
                        },
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(30.0),
                            ..Default::default()
                        },
                        TextColor(CYAN),
                    ));
                    if matches!(meter, Meter::Rubber) {
                        card.spawn((
                            Node {
                                width: percent(100.0),
                                height: px(6.0),
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgb(0.12, 0.18, 0.22)),
                        ))
                        .with_child((
                            RubberFill,
                            Node {
                                width: percent(100.0),
                                height: percent(100.0),
                                ..Default::default()
                            },
                            BackgroundColor(CYAN),
                        ));
                    }
                });
            }
        });
}

#[allow(clippy::type_complexity)]
pub fn update_hud(
    session: Res<Session>,
    menu: Res<MenuState>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    players: Query<
        (&Rubber, &SpeedMult, &IsAlive, Option<&ControlledBy>),
        (
            With<Player>,
            With<Controlled>,
            With<Predicted>,
            With<InputMarker<PlayerInput>>,
        ),
    >,
    mut roots: Query<&mut Node, With<HudRoot>>,
    mut fills: Query<(&mut Node, &mut BackgroundColor), (With<RubberFill>, Without<HudRoot>)>,
    mut values: Query<(&mut Text, &mut TextColor, &mut MeterValue)>,
) {
    let local = clients.single().ok().and_then(|client| {
        players
            .iter()
            .find(|(_, _, _, owner)| owner.is_none_or(|owner| owner.owner == client))
    });
    let visible = session.phase == ConnectionPhase::Playing
        && !menu.open
        && local.is_some_and(|(_, _, alive, _)| alive.0);
    for mut node in &mut roots {
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
    let Some((rubber, speed, _, _)) = local.filter(|_| visible) else {
        return;
    };
    let fraction = (rubber.0 / BASE_RUBBER).clamp(0.0, 1.0);
    let color = rubber_color(fraction);
    for (mut node, mut background) in &mut fills {
        node.width = percent(fraction * 100.0);
        background.0 = color;
    }
    for (mut text, mut text_color, mut value) in &mut values {
        let number = match value.meter {
            Meter::Rubber => (fraction * 100.0).round() as u32,
            Meter::Speed => (speed.0.max(0.0) * 100.0).round() as u32,
        };
        text_color.0 = match value.meter {
            Meter::Rubber => color,
            Meter::Speed => CYAN,
        };
        if value.displayed != Some(number) {
            text.0 = match value.meter {
                Meter::Rubber => format!("{number}%"),
                Meter::Speed => format!("{}.{:02}x", number / 100, number % 100),
            };
            value.displayed = Some(number);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::{PeerId, RemoteId};

    #[test]
    fn rubber_warning_thresholds_are_inclusive() {
        assert_eq!(rubber_color(0.51), CYAN);
        assert_eq!(rubber_color(0.5), Color::srgb(1.0, 0.75, 0.25));
        assert_eq!(rubber_color(0.21), Color::srgb(1.0, 0.75, 0.25));
        assert_eq!(rubber_color(0.2), Color::srgb(1.0, 0.3, 0.3));
        assert_eq!(rubber_color(0.0), Color::srgb(1.0, 0.3, 0.3));
    }

    #[test]
    fn meters_follow_local_prediction_and_session_lifecycle() {
        let mut world = World::new();
        world.init_resource::<Session>();
        world.init_resource::<MenuState>();
        world.run_system_once(setup_hud).unwrap();
        world.run_system_once(update_hud).unwrap();
        let root = world
            .query_filtered::<Entity, With<HudRoot>>()
            .single(&world)
            .unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);

        world.spawn((Client::default(), RemoteId(PeerId::Server), Connected));
        // Confirmed/remote copies must never supply the HUD values.
        world.spawn((Player, Rubber(1.0), SpeedMult(9.0), IsAlive(true)));
        let rider = world
            .spawn((
                Player,
                Controlled,
                Predicted,
                InputMarker::<PlayerInput>::default(),
                Rubber(BASE_RUBBER / 2.0),
                SpeedMult(1.25),
                IsAlive(true),
            ))
            .id();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.resource_mut::<MenuState>().open = false;
        world.run_system_once(update_hud).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::Flex);
        let labels: Vec<_> = world
            .query_filtered::<&Text, With<MeterValue>>()
            .iter(&world)
            .map(|text| text.0.as_str())
            .collect();
        assert!(labels.contains(&"50%"));
        assert!(labels.contains(&"1.25x"));

        for fraction in [1.0, 0.5, 0.2, 0.0] {
            world
                .entity_mut(rider)
                .insert(Rubber(BASE_RUBBER * fraction));
            world.run_system_once(update_hud).unwrap();
            let (node, color) = world
                .query_filtered::<(&Node, &BackgroundColor), With<RubberFill>>()
                .single(&world)
                .unwrap();
            assert_eq!(node.width, percent(fraction * 100.0));
            assert_eq!(color.0, rubber_color(fraction));
        }
        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(update_hud).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
        world.resource_mut::<MenuState>().open = false;
        world.entity_mut(rider).insert(IsAlive(false));
        world.run_system_once(update_hud).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
        world
            .entity_mut(rider)
            .insert((IsAlive(true), Rubber(BASE_RUBBER), SpeedMult(1.0)));
        world.run_system_once(update_hud).unwrap();
        let labels: Vec<_> = world
            .query_filtered::<&Text, With<MeterValue>>()
            .iter(&world)
            .map(|text| text.0.as_str())
            .collect();
        assert!(labels.contains(&"100%"));
        assert!(labels.contains(&"1.00x"));
        world.resource_mut::<Session>().phase = ConnectionPhase::Disconnecting;
        world.run_system_once(update_hud).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
        world.despawn(rider);
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.run_system_once(update_hud).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
    }
}
