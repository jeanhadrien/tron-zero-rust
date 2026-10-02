//! egui overlay menu; opening it captures controls but never pauses the server.

use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::connection::{ConnectionPhase, Session};
use crate::input::PendingInput;
use crate::settings::{MAX_KEYS_PER_SIDE, RebindState, TurnBindings, TurnSide, key_label};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuScreen {
    #[default]
    Main,
    Settings,
}

#[derive(Resource)]
pub struct MenuState {
    pub open: bool,
    pub block_this_frame: bool,
    pub screen: MenuScreen,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: true,
            block_this_frame: true,
            screen: MenuScreen::Main,
        }
    }
}

impl MenuState {
    pub fn captures_input(&self) -> bool {
        self.open || self.block_this_frame
    }
}

// Caption for the primary button; kept pure for tests.
pub fn primary_caption(phase: ConnectionPhase) -> &'static str {
    match phase {
        ConnectionPhase::Offline => "Connect to Localhost",
        ConnectionPhase::Connecting | ConnectionPhase::Synchronizing => "Cancel",
        ConnectionPhase::Playing => "Resume",
        ConnectionPhase::Disconnecting => "Disconnecting...",
    }
}

pub fn begin_input_frame(mut menu: ResMut<MenuState>) {
    menu.block_this_frame = false;
}

// Escape only (Tab+Enter focus activation replaces the old global Enter
// shortcut). A capture in progress eats the Escape instead of toggling.
pub fn menu_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuState>,
    session: Res<Session>,
    mut rebind: ResMut<RebindState>,
    mut pending: ResMut<PendingInput>,
) {
    // Escape takes precedence over other same-frame actions.
    if keys.just_pressed(KeyCode::Escape) {
        if rebind.capturing.is_some() {
            rebind.capturing = None;
            rebind.error = None;
        } else if session.phase == ConnectionPhase::Playing {
            menu.open = !menu.open;
        } else {
            menu.open = true;
        }
        menu.block_this_frame = true;
    }
    if menu.captures_input() {
        pending.0.clear();
    }
}

// Deferred UI action so binding lists can be read while drawing their buttons.
enum UiAction {
    None,
    Connect,
    CancelConnect,
    Resume,
    Disconnect,
    GotoSettings,
    Back,
    StartCapture(TurnSide, usize),
    RemoveKey(TurnSide, usize),
    ResetBindings,
}

impl UiAction {
    fn is_none(&self) -> bool {
        matches!(self, UiAction::None)
    }
}

pub fn menu_ui(
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut menu: ResMut<MenuState>,
    mut session: ResMut<Session>,
    mut bindings: ResMut<TurnBindings>,
    mut rebind: ResMut<RebindState>,
    time: Res<Time<Real>>,
) {
    if !menu.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // Snapshot what the panel reads; mutations apply after `show` returns.
    let screen = menu.screen;
    let phase = session.phase;
    let status = session.status.clone();
    let mut action = UiAction::None;
    egui::Window::new("TRON ZERO")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
        ui.vertical_centered(|ui| match screen {
            MenuScreen::Main => {
                ui.heading("TRON ZERO");
                ui.label("LOCALHOST\n127.0.0.1:5000");
                ui.label(status.as_str());
                let caption = primary_caption(phase);
                if ui
                    .add_enabled(
                        phase != ConnectionPhase::Disconnecting,
                        egui::Button::new(caption),
                    )
                    .clicked()
                    && action.is_none()
                {
                    action = match phase {
                        ConnectionPhase::Offline => UiAction::Connect,
                        ConnectionPhase::Connecting | ConnectionPhase::Synchronizing => {
                            UiAction::CancelConnect
                        }
                        ConnectionPhase::Playing => UiAction::Resume,
                        ConnectionPhase::Disconnecting => UiAction::None,
                    };
                }
                if phase == ConnectionPhase::Playing
                    && ui.button("Disconnect").clicked()
                    && action.is_none()
                {
                    action = UiAction::Disconnect;
                }
                if ui.button("Settings").clicked() && action.is_none() {
                    action = UiAction::GotoSettings;
                }
                ui.label(
                    "Escape: menu / resume\n\n\
                     Online play does not pause. While this menu is open,\n\
                     your rider keeps moving and can die.",
                );
            }
            MenuScreen::Settings => {
                ui.heading("Settings - Turn keys");
                for (side, title) in [(TurnSide::Left, "Left"), (TurnSide::Right, "Right")] {
                    ui.label(title);
                    ui.horizontal_wrapped(|ui| {
                        for (index, key) in bindings.keys(side).iter().enumerate() {
                            let label = if rebind.capturing == Some((side, index)) {
                                "press key...".to_owned()
                            } else {
                                key_label(*key)
                            };
                            if ui.button(label).clicked() && action.is_none() {
                                action = UiAction::StartCapture(side, index);
                            }
                            if ui.small_button("x").clicked() && action.is_none() {
                                action = UiAction::RemoveKey(side, index);
                            }
                        }
                    });
                    let len = bindings.keys(side).len();
                    if rebind.capturing == Some((side, len)) {
                        ui.label(format!("Press a key for {title}... (Escape cancels)"));
                    } else if ui
                        .add_enabled(
                            len < MAX_KEYS_PER_SIDE,
                            egui::Button::new("Add key"),
                        )
                        .clicked()
                        && action.is_none()
                    {
                        action = UiAction::StartCapture(side, len);
                    }
                }
                if let Some(error) = &rebind.error {
                    ui.colored_label(egui::Color32::RED, error.as_str());
                }
                if ui.button("Reset defaults").clicked() && action.is_none() {
                    action = UiAction::ResetBindings;
                }
                if ui.button("Back").clicked() && action.is_none() {
                    action = UiAction::Back;
                }
                ui.label("Keys apply immediately. Escape cancels capture.");
            }
        });
    });
    match action {
        UiAction::None => {}
        UiAction::Connect => {
            menu.block_this_frame = true;
            session.connect(
                SocketAddr::from((Ipv4Addr::LOCALHOST, 5000)),
                time.elapsed_secs_f64(),
                &mut commands,
            );
        }
        UiAction::CancelConnect | UiAction::Disconnect => {
            menu.block_this_frame = true;
            session.disconnect("Disconnected. Choose Localhost to reconnect.");
        }
        UiAction::Resume => {
            menu.block_this_frame = true;
            menu.open = false;
        }
        UiAction::GotoSettings => menu.screen = MenuScreen::Settings,
        UiAction::Back => menu.screen = MenuScreen::Main,
        UiAction::StartCapture(side, index) => {
            rebind.capturing = Some((side, index));
            rebind.error = None;
        }
        UiAction::RemoveKey(side, index) => {
            rebind.error = bindings.remove_key(side, index).err().map(str::to_owned);
        }
        UiAction::ResetBindings => {
            bindings.reset();
            rebind.capturing = None;
            rebind.error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn test_world() -> World {
        let mut world = World::new();
        world.insert_resource(MenuState {
            open: false,
            block_this_frame: false,
            screen: MenuScreen::Main,
        });
        world.init_resource::<Session>();
        world.init_resource::<RebindState>();
        world.init_resource::<Time<Real>>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.insert_resource(PendingInput([shared::PlayerInput::TurnLeft].into()));
        world
    }

    #[test]
    fn primary_captions_match_connection_phase() {
        assert_eq!(primary_caption(ConnectionPhase::Offline), "Connect to Localhost");
        assert_eq!(primary_caption(ConnectionPhase::Connecting), "Cancel");
        assert_eq!(primary_caption(ConnectionPhase::Synchronizing), "Cancel");
        assert_eq!(primary_caption(ConnectionPhase::Playing), "Resume");
        assert_eq!(
            primary_caption(ConnectionPhase::Disconnecting),
            "Disconnecting..."
        );
    }

    #[test]
    fn escape_clears_queue_and_captures_the_closing_frame() {
        let mut world = test_world();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
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
        let mut world = test_world();
        world.insert_resource(MenuState::default());
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        world.run_system_once(menu_controls).unwrap();
        assert!(world.resource::<MenuState>().open);
        assert_eq!(world.resource::<Session>().phase, ConnectionPhase::Offline);
    }

    #[test]
    fn escape_cancels_capture_instead_of_toggling() {
        let mut world = test_world();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.resource_mut::<RebindState>().capturing = Some((TurnSide::Left, 0));
        world.resource_mut::<RebindState>().error = Some("Key is already bound".into());
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        world.run_system_once(menu_controls).unwrap();
        assert!(world.resource::<RebindState>().capturing.is_none());
        assert!(world.resource::<RebindState>().error.is_none());
        assert!(!world.resource::<MenuState>().open);
        assert!(world.resource::<MenuState>().captures_input());
    }
}
