//! Persistent overlay; opening it captures controls but never pauses the server.

use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;

use crate::connection::{ConnectionPhase, Session};
use crate::input::PendingInput;

#[derive(Resource)]
pub struct MenuState {
    pub open: bool,
    pub block_this_frame: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: true,
            block_this_frame: true,
        }
    }
}

impl MenuState {
    pub fn captures_input(&self) -> bool {
        self.open || self.block_this_frame
    }
}

#[derive(Component)]
pub struct MenuRoot;

#[derive(Component)]
pub struct StatusLabel;

#[derive(Component, Clone, Copy)]
pub enum MenuAction {
    Primary,
    Disconnect,
}

#[derive(Component)]
pub struct ButtonLabel(MenuAction);

pub fn setup_menu(mut commands: Commands) {
    commands
        .spawn((
            MenuRoot,
            GlobalZIndex(100),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100.0),
                height: percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.01, 0.02, 0.04, 0.72)),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: px(460.0),
                    max_width: percent(95.0),
                    padding: UiRect::all(px(28.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(18.0),
                    ..Default::default()
                },
                BackgroundColor(Color::srgba(0.03, 0.06, 0.09, 0.96)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("TRON ZERO"),
                    TextFont {
                        font_size: FontSize::Px(36.0),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.0, 1.0, 0.8)),
                ));
                panel.spawn((
                    Text::new("LOCALHOST\n127.0.0.1:5000"),
                    TextFont {
                        font_size: FontSize::Px(22.0),
                        ..Default::default()
                    },
                ));
                panel.spawn((
                    StatusLabel,
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(18.0),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.75, 0.82, 0.88)),
                ));
                for action in [MenuAction::Primary, MenuAction::Disconnect] {
                    panel
                        .spawn((
                            Button,
                            action,
                            Node {
                                padding: UiRect::all(px(14.0)),
                                justify_content: JustifyContent::Center,
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgb(0.06, 0.22, 0.26)),
                        ))
                        .with_child((
                            ButtonLabel(action),
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(22.0),
                                ..Default::default()
                            },
                        ));
                }
                panel.spawn((
                    Text::new(
                        "Escape: menu / resume\nEnter: connect / cancel / resume\n\n\
                     Online play does not pause. While this menu is open,\n\
                     your rider keeps moving and can die.",
                    ),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(Color::srgb(0.65, 0.72, 0.78)),
                ));
            });
        });
}

pub fn begin_input_frame(mut menu: ResMut<MenuState>) {
    menu.block_this_frame = false;
}

pub fn menu_controls(
    keys: Res<ButtonInput<KeyCode>>,
    interactions: Query<(&Interaction, &MenuAction), Changed<Interaction>>,
    mut menu: ResMut<MenuState>,
    mut session: ResMut<Session>,
    mut pending: ResMut<PendingInput>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    // Escape takes precedence over other same-frame actions.
    if keys.just_pressed(KeyCode::Escape) {
        if session.phase == ConnectionPhase::Playing {
            menu.open = !menu.open;
        } else {
            menu.open = true;
        }
        menu.block_this_frame = true;
    } else if menu.open {
        let action = interactions
            .iter()
            .find(|(interaction, _)| **interaction == Interaction::Pressed)
            .map(|(_, action)| *action)
            .or_else(|| {
                (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
                    .then_some(MenuAction::Primary)
            });
        if let Some(action) = action {
            menu.block_this_frame = true;
            match (action, session.phase) {
                (MenuAction::Primary, ConnectionPhase::Offline) => {
                    session.connect(
                        SocketAddr::from((Ipv4Addr::LOCALHOST, 5000)),
                        time.elapsed_secs_f64(),
                        &mut commands,
                    );
                }
                (MenuAction::Primary, ConnectionPhase::Playing) => menu.open = false,
                (
                    MenuAction::Primary,
                    ConnectionPhase::Connecting | ConnectionPhase::Synchronizing,
                )
                | (MenuAction::Disconnect, ConnectionPhase::Playing) => {
                    session.disconnect("Disconnected. Choose Localhost to reconnect.");
                }
                _ => {}
            }
        }
    }
    if menu.captures_input() {
        pending.0.clear();
    }
}

pub fn update_menu(
    menu: Res<MenuState>,
    session: Res<Session>,
    mut root: Query<&mut Node, With<MenuRoot>>,
    mut status: Query<&mut Text, (With<StatusLabel>, Without<ButtonLabel>)>,
    mut labels: Query<(&ButtonLabel, &mut Text), Without<StatusLabel>>,
    mut buttons: Query<
        (&MenuAction, &Interaction, &mut BackgroundColor, &mut Node),
        Without<MenuRoot>,
    >,
) {
    for mut node in &mut root {
        node.display = if menu.open {
            Display::Flex
        } else {
            Display::None
        };
    }
    for mut text in &mut status {
        if text.0 != session.status {
            text.0.clone_from(&session.status);
        }
    }
    for (label, mut text) in &mut labels {
        let caption = match label.0 {
            MenuAction::Primary => match session.phase {
                ConnectionPhase::Offline => "Connect to Localhost",
                ConnectionPhase::Connecting | ConnectionPhase::Synchronizing => "Cancel",
                ConnectionPhase::Playing => "Resume",
                ConnectionPhase::Disconnecting => "Disconnecting...",
            },
            MenuAction::Disconnect => "Disconnect",
        };
        if text.0 != caption {
            text.0 = caption.into();
        }
    }
    for (action, interaction, mut color, mut node) in &mut buttons {
        node.display = if matches!(action, MenuAction::Disconnect)
            && session.phase != ConnectionPhase::Playing
        {
            Display::None
        } else {
            Display::Flex
        };
        color.0 = match interaction {
            Interaction::Pressed => Color::srgb(0.1, 0.5, 0.48),
            Interaction::Hovered => Color::srgb(0.08, 0.34, 0.38),
            Interaction::None => Color::srgb(0.06, 0.22, 0.26),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn escape_clears_queue_and_captures_the_closing_frame() {
        let mut world = World::new();
        world.insert_resource(MenuState {
            open: false,
            block_this_frame: false,
        });
        world.init_resource::<Session>();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.init_resource::<Time<Real>>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.insert_resource(PendingInput([shared::PlayerInput::TurnLeft].into()));
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        world.run_system_once(menu_controls).unwrap();
        assert!(world.resource::<MenuState>().open);
        assert!(world.resource::<PendingInput>().0.is_empty());
        world.run_system_once(begin_input_frame).unwrap();
        world.run_system_once(menu_controls).unwrap();
        assert!(!world.resource::<MenuState>().open);
        assert!(world.resource::<MenuState>().captures_input());
        world.run_system_once(begin_input_frame).unwrap();
        assert!(!world.resource::<MenuState>().captures_input());
    }

    #[test]
    fn escape_cannot_hide_disconnected_menu() {
        let mut world = World::new();
        world.init_resource::<MenuState>();
        world.init_resource::<Session>();
        world.init_resource::<Time<Real>>();
        world.init_resource::<PendingInput>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        world.run_system_once(menu_controls).unwrap();
        assert!(world.resource::<MenuState>().open);
        assert_eq!(world.resource::<Session>().phase, ConnectionPhase::Offline);
    }
}
