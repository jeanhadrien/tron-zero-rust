//! egui overlay menu; opening it captures controls but never pauses the server.

use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::browser::{BrowserState, FetchStatus, ManagerConfig, connect_supported};
use crate::connection::{ConnectionPhase, Session};
use crate::input::PendingInput;
use crate::settings::{MAX_KEYS_PER_SIDE, RebindState, TurnBindings, TurnSide, key_label};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuScreen {
    #[default]
    Main,
    Settings,
    Rooms,
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
        } else if menu.screen == MenuScreen::Rooms {
            menu.screen = MenuScreen::Main;
            menu.open = true;
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
    GotoRooms,
    Back,
    Refresh,
    ConnectRoom(SocketAddr),
    StartCapture(TurnSide, usize),
    RemoveKey(TurnSide, usize),
    ResetBindings,
}

impl UiAction {
    fn is_none(&self) -> bool {
        matches!(self, UiAction::None)
    }
}

// Theme lives in `crate::theme` so menu, HUD, and death prompt share one
// palette: near-black navy panel over a dimmed fullscreen backdrop, cyan
// headings, teal full-width action buttons.

// Bevy injects each resource as its own parameter; the count is structural.
#[allow(clippy::too_many_arguments)]
pub fn menu_ui(
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut menu: ResMut<MenuState>,
    mut session: ResMut<Session>,
    mut bindings: ResMut<TurnBindings>,
    mut rebind: ResMut<RebindState>,
    mut browser: ResMut<BrowserState>,
    config: Res<ManagerConfig>,
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
    // Fullscreen dim backdrop, then a borderless panel centered on screen.
    let viewport = ctx.viewport_rect();
    egui::Area::new("menu_dim".into())
        .order(egui::Order::Background)
        .fixed_pos(viewport.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(viewport, 0.0, theme::dim());
        });
    egui::Area::new("menu_panel".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            theme::panel_frame().show(ui, |ui| {
                ui.set_min_width(404.0);
                theme::theme_panel(ui);
                ui.vertical_centered(|ui| match screen {
                    MenuScreen::Main => {
                        ui.label(
                            egui::RichText::new("TRON ZERO")
                                .size(34.0)
                                .strong()
                                .color(theme::CYAN),
                        );
                        ui.label(egui::RichText::new("LOCALHOST\n127.0.0.1:5000").size(20.0));
                        ui.label(
                            egui::RichText::new(status.as_str())
                                .size(16.0)
                                .color(theme::BODY),
                        );
                        ui.separator();
                        let caption = primary_caption(phase);
                        let primary = ui
                            .add_enabled_ui(phase != ConnectionPhase::Disconnecting, |ui| {
                                theme::action_button(ui, caption)
                            });
                        if primary.inner.clicked() && action.is_none() {
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
                            && theme::action_button(ui, "Disconnect").clicked()
                            && action.is_none()
                        {
                            action = UiAction::Disconnect;
                        }
                        if theme::action_button(ui, "Settings").clicked() && action.is_none() {
                            action = UiAction::GotoSettings;
                        }
                        if theme::action_button(ui, "Browse Servers").clicked()
                            && action.is_none()
                        {
                            action = UiAction::GotoRooms;
                        }
                    }
                    MenuScreen::Settings => {
                        ui.label(
                            egui::RichText::new("SETTINGS")
                                .size(26.0)
                                .strong()
                                .color(theme::CYAN),
                        );
                        ui.separator();
                        for (side, title) in [(TurnSide::Left, "Left"), (TurnSide::Right, "Right")]
                        {
                            ui.label(
                                egui::RichText::new(title)
                                    .size(16.0)
                                    .strong()
                                    .color(theme::BODY),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for (index, binding) in bindings.keys(side).iter().enumerate() {
                                    let label = if rebind.capturing == Some((side, index)) {
                                        "press key...".to_owned()
                                    } else {
                                        key_label(binding).to_owned()
                                    };
                                    if ui
                                        .button(
                                            egui::RichText::new(label)
                                                .family(egui::FontFamily::Monospace)
                                                .size(18.0),
                                        )
                                        .clicked()
                                        && action.is_none()
                                    {
                                        action = UiAction::StartCapture(side, index);
                                    }
                                    if ui.small_button("x").clicked() && action.is_none() {
                                        action = UiAction::RemoveKey(side, index);
                                    }
                                }
                            });
                            let len = bindings.keys(side).len();
                            if rebind.capturing == Some((side, len)) {
                                ui.label(
                                    egui::RichText::new(format!("Press a key for {title}..."))
                                        .color(theme::BODY),
                                );
                            } else if ui
                                .add_enabled(len < MAX_KEYS_PER_SIDE, egui::Button::new("Add key"))
                                .clicked()
                                && action.is_none()
                            {
                                action = UiAction::StartCapture(side, len);
                            }
                        }
                        if let Some(error) = &rebind.error {
                            ui.colored_label(egui::Color32::RED, error.as_str());
                        }
                        ui.separator();
                        if theme::action_button(ui, "Reset defaults").clicked() && action.is_none()
                        {
                            action = UiAction::ResetBindings;
                        }
                        if theme::action_button(ui, "Back").clicked() && action.is_none() {
                            action = UiAction::Back;
                        }
                    }
                    MenuScreen::Rooms => {
                        ui.label(
                            egui::RichText::new("SERVERS")
                                .size(26.0)
                                .strong()
                                .color(theme::CYAN),
                        );
                        ui.label(
                            egui::RichText::new(config.base_url.as_str())
                                .size(14.0)
                                .color(theme::BODY),
                        );
                        match browser.status {
                            FetchStatus::Loading => {
                                ui.label(
                                    egui::RichText::new("Loading rooms...")
                                        .size(14.0)
                                        .color(theme::BODY),
                                );
                            }
                            FetchStatus::Loaded => {
                                let count = browser.rows.len();
                                let noun = if count == 1 { "room" } else { "rooms" };
                                let line = match browser.last_updated {
                                    Some(updated) => format!(
                                        "{count} {noun} - updated {:.0}s ago",
                                        (time.elapsed_secs_f64() - updated).max(0.0)
                                    ),
                                    None => format!("{count} {noun}"),
                                };
                                ui.label(
                                    egui::RichText::new(line).size(14.0).color(theme::BODY),
                                );
                            }
                            FetchStatus::Failed => {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    browser.error.as_deref().unwrap_or("Room fetch failed."),
                                );
                            }
                        }
                        if ui
                            .add_enabled(
                                browser.status != FetchStatus::Loading,
                                egui::Button::new("Refresh"),
                            )
                            .clicked()
                            && action.is_none()
                        {
                            action = UiAction::Refresh;
                        }
                        if !connect_supported() {
                            ui.label(
                                egui::RichText::new(
                                    "Listing only: connecting needs a native build (WebTransport pending).",
                                )
                                .size(13.0)
                                .color(theme::BODY),
                            );
                        }
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            if browser.status == FetchStatus::Loaded
                                && browser.rows.iter().all(|row| row.is_local)
                            {
                                // Local-only rows mean either an empty manager or
                                // one holding just our own server (deduped above):
                                // say which, so registration reads as success.
                                let empty = if browser.manager_total == 0 {
                                    "No rooms found. Start a server or check the manager URL."
                                } else {
                                    "Your server is registered - connect via Localhost below."
                                };
                                ui.label(
                                    egui::RichText::new(empty)
                                    .size(14.0)
                                    .color(theme::BODY),
                                );
                            }
                            for row in &browser.rows {
                                theme::card_frame().show(ui, |ui| {
                                    // Pinned local row reads as the preselected default;
                                    // connecting still takes one explicit click.
                                    let name_color =
                                        if row.is_local { theme::CYAN } else { theme::BODY };
                                    ui.label(
                                        egui::RichText::new(row.room.display_name.as_str())
                                            .size(18.0)
                                            .strong()
                                            .color(name_color),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}/{} players - {}:{}",
                                            row.room.player_count,
                                            row.room.max_players,
                                            row.room.host,
                                            row.room.port
                                        ))
                                        .size(14.0)
                                        .color(theme::BODY),
                                    );
                                    if let Some(error) = &row.resolve_error {
                                        ui.colored_label(egui::Color32::RED, error.as_str());
                                    }
                                    let can_connect = row.addr.is_some()
                                        && phase == ConnectionPhase::Offline
                                        && connect_supported();
                                    if ui
                                        .add_enabled(can_connect, egui::Button::new("Connect"))
                                        .clicked()
                                        && action.is_none()
                                        && let Some(addr) = row.addr
                                    {
                                        action = UiAction::ConnectRoom(addr);
                                    }
                                });
                            }
                        });
                        if theme::action_button(ui, "Back").clicked() && action.is_none() {
                            action = UiAction::Back;
                        }
                    }
                });
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
        UiAction::GotoRooms => {
            menu.screen = MenuScreen::Rooms;
            browser.request_refresh(&config);
        }
        UiAction::Refresh => browser.request_refresh(&config),
        UiAction::ConnectRoom(addr) => {
            if session.phase == ConnectionPhase::Offline {
                menu.screen = MenuScreen::Main;
                menu.block_this_frame = true;
                session.connect(addr, time.elapsed_secs_f64(), &mut commands);
            }
        }
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
        assert_eq!(
            primary_caption(ConnectionPhase::Offline),
            "Connect to Localhost"
        );
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
