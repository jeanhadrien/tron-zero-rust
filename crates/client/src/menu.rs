//! egui overlay menu; opening it captures controls but never pauses the server.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::account::{Accounts, RankedJoin, Standing};
use crate::browser::{self, BrowserRow, BrowserState, FetchStatus, ManagerConfig};
use crate::connection::{AutoConnect, ConnectionPhase, Endpoint, IDLE_STATUS, Session};
use crate::input::PendingInput;
use crate::local_scan::LocalServers;
use crate::render::FxSettings;
use crate::settings::{
    MAX_KEYS_PER_SIDE, PlayerName, RebindState, TurnBindings, TurnSide, key_label,
};
use crate::theme::{self, Theme, ThemeId};
use shared::protocol::{MAX_NAME_CHARS, sanitize_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuScreen {
    #[default]
    Main,
    Settings,
    Rooms,
    Leaderboard,
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

// Caption for the primary button; kept pure for tests. `local_found`: a server
// answered on this machine, so there is something to connect to while offline.
pub fn primary_caption(phase: ConnectionPhase, local_found: bool) -> &'static str {
    match phase {
        ConnectionPhase::Offline if local_found => "Connect to Localhost",
        ConnectionPhase::Offline => "No local server found",
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
        } else if matches!(menu.screen, MenuScreen::Rooms | MenuScreen::Leaderboard) {
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
    Connect(Endpoint),
    CancelConnect,
    Resume,
    Disconnect,
    Quit,
    GotoSettings,
    GotoRooms,
    GotoLeaderboard,
    /// Play now: the manager picks a ranked room.
    PlayRanked,
    Back,
    Refresh,
    ConnectRoom(Endpoint),
    /// A ranked room: sign in and fetch a join ticket first.
    ConnectRanked(Endpoint, String),
    StartCapture(TurnSide, usize),
    RemoveKey(TurnSide, usize),
    ResetBindings,
    SetTheme(ThemeId),
    SetEffects(bool),
}

impl UiAction {
    fn is_none(&self) -> bool {
        matches!(self, UiAction::None)
    }

    /// Records `next` unless a widget already claimed this frame's action.
    fn set_once(&mut self, next: UiAction) {
        if self.is_none() {
            *self = next;
        }
    }
}

// Theme lives in `crate::theme` so menu, HUD, and death prompt share one
// palette: near-black navy panel over a dimmed fullscreen backdrop, cyan
// headings, teal full-width action buttons.

fn heading(ui: &mut egui::Ui, theme: &Theme, text: &str, size: f32) {
    ui.label(
        egui::RichText::new(text)
            .size(size)
            .strong()
            .color(theme.accent.egui()),
    );
}

fn body_label(ui: &mut egui::Ui, theme: &Theme, text: impl Into<String>, size: f32) {
    ui.label(
        egui::RichText::new(text)
            .size(size)
            .color(theme.body.egui()),
    );
}

// One argument per thing the screen draws; a struct would only rename them.
#[allow(clippy::too_many_arguments)]
fn main_screen(
    ui: &mut egui::Ui,
    theme: &Theme,
    phase: ConnectionPhase,
    status: &str,
    name_edit: &mut String,
    local: Option<&Endpoint>,
    rating: Option<&str>,
    action: &mut UiAction,
) {
    heading(ui, theme, "TRON ZERO", 34.0);
    body_label(ui, theme, status, 16.0);
    if let Some(rating) = rating {
        body_label(ui, theme, rating, 14.0);
    }
    ui.separator();
    ui.horizontal(|ui| {
        body_label(ui, theme, "Name", 16.0);
        ui.add_enabled(
            phase == ConnectionPhase::Offline,
            egui::TextEdit::singleline(name_edit)
                .char_limit(MAX_NAME_CHARS)
                .hint_text(shared::protocol::DEFAULT_NAME)
                .desired_width(f32::INFINITY),
        );
    });
    let primary_enabled = match phase {
        ConnectionPhase::Disconnecting => false,
        ConnectionPhase::Offline => local.is_some(),
        _ => true,
    };
    let primary = ui.add_enabled_ui(primary_enabled, |ui| {
        theme::action_button(ui, primary_caption(phase, local.is_some()))
    });
    if primary.inner.clicked() {
        action.set_once(match (phase, local) {
            (ConnectionPhase::Offline, Some(endpoint)) => UiAction::Connect(endpoint.clone()),
            (ConnectionPhase::Offline, None) => UiAction::None,
            (ConnectionPhase::Connecting | ConnectionPhase::Synchronizing, _) => {
                UiAction::CancelConnect
            }
            (ConnectionPhase::Playing, _) => UiAction::Resume,
            (ConnectionPhase::Disconnecting, _) => UiAction::None,
        });
    }
    if phase == ConnectionPhase::Playing && theme::action_button(ui, "Disconnect").clicked() {
        action.set_once(UiAction::Disconnect);
    }
    if theme::action_button(ui, "Settings").clicked() {
        action.set_once(UiAction::GotoSettings);
    }
    if theme::action_button(ui, "Browse Servers").clicked() {
        action.set_once(UiAction::GotoRooms);
    }
    if phase == ConnectionPhase::Offline && theme::action_button(ui, "Play ranked").clicked() {
        action.set_once(UiAction::PlayRanked);
    }
    if theme::action_button(ui, "Leaderboard").clicked() {
        action.set_once(UiAction::GotoLeaderboard);
    }
    if crate::platform::CAN_QUIT && theme::action_button(ui, "Quit game").clicked() {
        action.set_once(UiAction::Quit);
    }
}

fn settings_screen(
    ui: &mut egui::Ui,
    theme: &Theme,
    effects: bool,
    bindings: &TurnBindings,
    rebind: &RebindState,
    action: &mut UiAction,
) {
    heading(ui, theme, "SETTINGS", 26.0);
    ui.separator();
    ui.label(
        egui::RichText::new("Theme")
            .size(16.0)
            .strong()
            .color(theme.body.egui()),
    );
    ui.horizontal_wrapped(|ui| {
        for id in ThemeId::ALL {
            if ui.selectable_label(theme.id == id, id.label()).clicked() {
                action.set_once(UiAction::SetTheme(id));
            }
        }
    });
    let mut effects_on = effects;
    if ui.checkbox(&mut effects_on, "Particle effects").changed() {
        action.set_once(UiAction::SetEffects(effects_on));
    }
    ui.separator();
    for (side, title) in [(TurnSide::Left, "Left"), (TurnSide::Right, "Right")] {
        ui.label(
            egui::RichText::new(title)
                .size(16.0)
                .strong()
                .color(theme.body.egui()),
        );
        ui.horizontal_wrapped(|ui| {
            for (index, binding) in bindings.keys(side).iter().enumerate() {
                let label = if rebind.capturing == Some((side, index)) {
                    "press key...".to_owned()
                } else {
                    key_label(binding).to_owned()
                };
                let key_button = egui::RichText::new(label)
                    .family(egui::FontFamily::Monospace)
                    .size(18.0);
                if ui.button(key_button).clicked() {
                    action.set_once(UiAction::StartCapture(side, index));
                }
                if ui.small_button("x").clicked() {
                    action.set_once(UiAction::RemoveKey(side, index));
                }
            }
        });
        let len = bindings.keys(side).len();
        if rebind.capturing == Some((side, len)) {
            ui.label(
                egui::RichText::new(format!("Press a key for {title}...")).color(theme.body.egui()),
            );
        } else if ui
            .add_enabled(len < MAX_KEYS_PER_SIDE, egui::Button::new("Add key"))
            .clicked()
        {
            action.set_once(UiAction::StartCapture(side, len));
        }
    }
    if let Some(error) = &rebind.error {
        ui.colored_label(theme.danger.egui(), error.as_str());
    }
    ui.separator();
    if theme::action_button(ui, "Reset defaults").clicked() {
        action.set_once(UiAction::ResetBindings);
    }
    if theme::action_button(ui, "Back").clicked() {
        action.set_once(UiAction::Back);
    }
}

// One parameter per piece of state the screen draws; grouping them would only
// add a struct used in a single place.
#[allow(clippy::too_many_arguments)]
fn rooms_screen(
    ui: &mut egui::Ui,
    theme: &Theme,
    browser: &BrowserState,
    rows: &[BrowserRow],
    config: &ManagerConfig,
    joinable: bool,
    now: f64,
    action: &mut UiAction,
) {
    heading(ui, theme, "SERVERS", 26.0);
    body_label(ui, theme, config.base_url.as_str(), 14.0);
    match browser.status {
        FetchStatus::Loading => body_label(ui, theme, "Loading rooms...", 14.0),
        FetchStatus::Loaded => {
            let count = browser.rows.len();
            let noun = if count == 1 { "room" } else { "rooms" };
            let line = match browser.last_updated {
                Some(updated) => format!(
                    "{count} {noun} - updated {:.0}s ago",
                    (now - updated).max(0.0)
                ),
                None => format!("{count} {noun}"),
            };
            body_label(ui, theme, line, 14.0);
        }
        FetchStatus::Failed => {
            ui.colored_label(
                theme.danger.egui(),
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
    {
        action.set_once(UiAction::Refresh);
    }
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            if browser.status == FetchStatus::Loaded && rows.iter().all(|row| row.is_local) {
                // Local-only rows mean either an empty manager or one holding
                // just our own server (deduped): say which, so registration
                // reads as success.
                let empty = if browser.manager_total == 0 {
                    "No rooms found. Start a server or check the manager URL."
                } else {
                    "Your server is registered - it is listed above as Localhost."
                };
                body_label(ui, theme, empty, 14.0);
            }
            for row in rows {
                room_card(ui, theme, row, joinable, action);
            }
        });
    if theme::action_button(ui, "Back").clicked() {
        action.set_once(UiAction::Back);
    }
}

fn room_card(
    ui: &mut egui::Ui,
    theme: &Theme,
    row: &BrowserRow,
    joinable: bool,
    action: &mut UiAction,
) {
    theme.card_frame().show(ui, |ui| {
        // Pinned local row reads as the preselected default; connecting still
        // takes one explicit click.
        let name_color = if row.is_local {
            theme.accent
        } else {
            theme.body
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(row.room.display_name.as_str())
                    .size(18.0)
                    .strong()
                    .color(name_color.egui()),
            );
            if row.room.ranked {
                ui.label(
                    egui::RichText::new("RANKED")
                        .size(12.0)
                        .strong()
                        .color(theme.accent.egui()),
                );
            }
        });
        let bots = match row.room.bot_count {
            0 => String::new(),
            n => format!(" (+{n} bots)"),
        };
        body_label(
            ui,
            theme,
            format!(
                "{}/{} players{} - {}:{}",
                row.room.player_count,
                row.room.max_players,
                bots,
                row.room.host,
                row.play_port()
            ),
            14.0,
        );
        if let Some(problem) = &row.problem {
            ui.colored_label(theme.danger.egui(), problem.as_str());
        }
        let full = row.room.is_full();
        let can_connect = row.endpoint.is_some() && joinable && !full;
        let caption = if full { "Full" } else { "Connect" };
        if ui
            .add_enabled(can_connect, egui::Button::new(caption))
            .clicked()
            && let Some(endpoint) = &row.endpoint
        {
            action.set_once(if row.room.ranked {
                UiAction::ConnectRanked(endpoint.clone(), row.room.id.clone())
            } else {
                UiAction::ConnectRoom(endpoint.clone())
            });
        }
    });
}

fn leaderboard_screen(
    ui: &mut egui::Ui,
    theme: &Theme,
    standing: &Standing,
    rating: Option<&str>,
    action: &mut UiAction,
) {
    heading(ui, theme, "LEADERBOARD", 26.0);
    body_label(
        ui,
        theme,
        rating.unwrap_or("Play a ranked server to get a rating."),
        14.0,
    );
    if let Some(error) = &standing.error {
        ui.colored_label(theme.danger.egui(), error.as_str());
    } else if standing.loading() {
        body_label(ui, theme, "Loading...", 14.0);
    }
    if ui
        .add_enabled(!standing.loading(), egui::Button::new("Refresh"))
        .clicked()
    {
        action.set_once(UiAction::Refresh);
    }
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            for row in &standing.board {
                let bot = if row.kind == "bot" { " (bot)" } else { "" };
                body_label(
                    ui,
                    theme,
                    format!(
                        "#{:<3} {:>5.0}  {}{bot}",
                        row.rank, row.rating, row.display_name
                    ),
                    15.0,
                );
            }
        });
    if theme::action_button(ui, "Back").clicked() {
        action.set_once(UiAction::Back);
    }
}

/// The ranked-play resources the menu reads and starts joins with (grouped to
/// stay under Bevy's system parameter limit).
#[derive(SystemParam)]
pub struct RankedUi<'w> {
    join: ResMut<'w, RankedJoin>,
    standing: ResMut<'w, Standing>,
    accounts: Res<'w, Accounts>,
}

// Bevy injects each resource as its own parameter; the count is structural.
#[allow(clippy::too_many_arguments)]
pub fn menu_ui(
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut menu: ResMut<MenuState>,
    mut session: ResMut<Session>,
    mut bindings: ResMut<TurnBindings>,
    mut player_name: ResMut<PlayerName>,
    mut auto: Option<ResMut<AutoConnect>>,
    mut rebind: ResMut<RebindState>,
    mut theme: ResMut<Theme>,
    mut fx: ResMut<FxSettings>,
    mut browser: ResMut<BrowserState>,
    ranked_ui: RankedUi,
    config: Res<ManagerConfig>,
    servers: Res<LocalServers>,
    time: Res<Time<Real>>,
) {
    if !menu.open {
        return;
    }
    let RankedUi {
        join: mut ranked,
        mut standing,
        accounts,
    } = ranked_ui;
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // Snapshot what the panel reads; mutations apply after `show` returns.
    let screen = menu.screen;
    let phase = session.phase;
    let status = session.status.clone();
    let now = time.elapsed_secs_f64();
    let mut action = UiAction::None;
    let joinable = phase == ConnectionPhase::Offline && !ranked.busy();
    let rating = standing.me.as_ref().map(|me| me.summary());
    let local_rows = browser::local_rows(&servers);
    let local_endpoint = local_rows.iter().find_map(|row| row.endpoint.clone());
    let rows = if screen == MenuScreen::Rooms {
        browser::merge_rows(local_rows, &browser.rows)
    } else {
        Vec::new()
    };
    // Edited on a copy so the resource is only marked changed (and saved)
    // when the text really differs.
    let mut name_edit = player_name.0.clone();
    // Fullscreen dim backdrop, then a borderless panel centered on screen.
    let viewport = ctx.viewport_rect();
    egui::Area::new("menu_dim".into())
        .order(egui::Order::Background)
        .fixed_pos(viewport.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(viewport, 0.0, theme.dim());
        });
    egui::Area::new("menu_panel".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            theme.panel_frame().show(ui, |ui| {
                ui.set_min_width(404.0);
                theme::panel_spacing(ui);
                ui.vertical_centered(|ui| match screen {
                    MenuScreen::Main => main_screen(
                        ui,
                        &theme,
                        phase,
                        &status,
                        &mut name_edit,
                        local_endpoint.as_ref(),
                        rating.as_deref(),
                        &mut action,
                    ),
                    MenuScreen::Settings => {
                        settings_screen(ui, &theme, fx.enabled, &bindings, &rebind, &mut action)
                    }
                    MenuScreen::Rooms => rooms_screen(
                        ui,
                        &theme,
                        &browser,
                        &rows,
                        &config,
                        joinable,
                        now,
                        &mut action,
                    ),
                    MenuScreen::Leaderboard => {
                        leaderboard_screen(ui, &theme, &standing, rating.as_deref(), &mut action)
                    }
                });
            });
        });
    if name_edit != player_name.0 {
        player_name.0 = name_edit;
    }
    // Whatever the player does by hand takes over from TRON_AUTOCONNECT.
    if !action.is_none()
        && let Some(auto) = auto.as_mut()
    {
        auto.disarm();
    }
    let join_name = sanitize_name(&player_name.0);
    match action {
        UiAction::None => {}
        UiAction::Connect(endpoint) => {
            menu.block_this_frame = true;
            session.connect(endpoint, &join_name, now, &mut commands);
        }
        UiAction::CancelConnect => {
            menu.block_this_frame = true;
            session.disconnect(IDLE_STATUS);
        }
        UiAction::Disconnect => {
            menu.block_this_frame = true;
            session.disconnect("Disconnected from server.");
        }
        UiAction::Quit => session.request_quit(now),
        UiAction::Resume => {
            menu.block_this_frame = true;
            menu.open = false;
        }
        UiAction::GotoSettings => menu.screen = MenuScreen::Settings,
        UiAction::GotoRooms => {
            menu.screen = MenuScreen::Rooms;
            browser.request_refresh(&config);
        }
        UiAction::GotoLeaderboard => {
            menu.screen = MenuScreen::Leaderboard;
            standing.refresh(&config, &accounts);
        }
        UiAction::Refresh if screen == MenuScreen::Leaderboard => {
            standing.refresh(&config, &accounts);
        }
        UiAction::Refresh => browser.request_refresh(&config),
        UiAction::ConnectRoom(endpoint) => {
            if session.phase == ConnectionPhase::Offline {
                menu.screen = MenuScreen::Main;
                menu.block_this_frame = true;
                session.connect(endpoint, &join_name, now, &mut commands);
            }
        }
        UiAction::ConnectRanked(endpoint, room_id) => {
            if joinable {
                menu.screen = MenuScreen::Main;
                menu.block_this_frame = true;
                session.status = "Signing in to the ranked server...".into();
                ranked.start(endpoint, room_id, join_name, &config, &accounts, now);
            }
        }
        UiAction::PlayRanked => {
            if joinable {
                menu.block_this_frame = true;
                // After a refusal (full, bad ticket), ask for a different room.
                let exclude = if session.join_rejected() {
                    ranked.last_placed.iter().cloned().collect()
                } else {
                    Vec::new()
                };
                session.status = "Finding a ranked server...".into();
                ranked.start_play(exclude, join_name, &config, &accounts, now);
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
        UiAction::SetTheme(id) => {
            // Skip a re-click on the active theme: replacing the resource
            // would re-apply the style and rewrite the config for nothing.
            if theme.id != id {
                *theme = Theme::from_id(id);
            }
        }
        UiAction::SetEffects(enabled) => fx.enabled = enabled,
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
        world.insert_resource(PendingInput(
            [crate::input::keyboard::PendingPress {
                direction: shared::protocol::TurnDirection::Left,
                generation: 0,
            }]
            .into(),
        ));
        world
    }

    #[test]
    fn primary_captions_match_connection_phase() {
        assert_eq!(
            primary_caption(ConnectionPhase::Offline, true),
            "Connect to Localhost"
        );
        assert_eq!(
            primary_caption(ConnectionPhase::Offline, false),
            "No local server found"
        );
        assert_eq!(
            primary_caption(ConnectionPhase::Connecting, false),
            "Cancel"
        );
        assert_eq!(
            primary_caption(ConnectionPhase::Synchronizing, false),
            "Cancel"
        );
        assert_eq!(primary_caption(ConnectionPhase::Playing, false), "Resume");
        assert_eq!(
            primary_caption(ConnectionPhase::Disconnecting, false),
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
