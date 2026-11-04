//! Tron Zero — client entry point.
//!
//! Connects to a lightyear server via UDP / raw connection, sends keyboard
//! inputs, predicts the local lightcycle, and renders the arena + players.

use bevy::prelude::*;
use bevy::window::WindowCloseRequested;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use lightyear::prelude::*;
use tron_zero_client::{
    add_input_and_prediction, add_network, attach_local_input, browser, config, connection, input,
    local_scan, menu, render, settings, sim, theme,
};

fn main() {
    let mut app = App::new();

    // Closing the window goes through the same clean quit as the menu button,
    // so the server gets the leave before the process exits.
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(primary_window()),
        close_when_requested: false,
        ..default()
    }));
    app.add_plugins(EguiPlugin::default());
    add_network(&mut app);

    // Saved preferences (name, key bindings, manager URL); never fatal.
    let (preferences, saved) = config::Preferences::load();
    app.insert_resource(saved.name());
    app.insert_resource(saved.bindings());
    app.insert_resource(saved.theme());
    app.insert_resource(browser::ManagerConfig::from_env_and_saved(
        saved.manager_url.clone(),
    ));
    app.insert_resource(preferences);
    app.add_systems(Last, config::persist_preferences);

    // Opt-in dev helpers: simulated network conditions and auto-join.
    if let Some(net_sim) = sim::NetSim::from_env() {
        net_sim.install(&mut app);
    }
    if let Some(auto) = connection::AutoConnect::from_env() {
        app.insert_resource(auto);
    }
    // A quit requested this frame must be seen before auto-connect can start a
    // new session.
    app.add_systems(Update, connection::auto_connect);
    app.add_systems(
        Update,
        request_quit_on_window_close.before(connection::auto_connect),
    );
    app.add_systems(
        Update,
        connection::quit_when_ready.after(connection::finish_disconnect),
    );

    app.init_resource::<connection::Session>();
    app.init_resource::<menu::MenuState>();
    app.init_resource::<browser::BrowserState>();
    app.init_resource::<local_scan::LocalServers>();
    app.add_systems(PreUpdate, local_scan::scan_local_servers);
    app.add_systems(Startup, browser::initial_refresh);
    app.add_systems(PreUpdate, browser::poll_browser_fetch);
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
            .after(MessageSystems::Receive)
            .after(ReplicationSystems::Receive)
            .before(input::buffer_keyboard_input),
    );
    app.add_systems(Update, connection::finish_disconnect);
    // egui overlays share one pass; order keeps the menu above HUD/death on
    // any same-order tie (menu.open also hides both outright).
    app.add_systems(
        EguiPrimaryContextPass,
        (
            theme::apply_egui_theme,
            (
                render::hud_ui,
                render::status_badge_ui,
                render::death_ui,
                render::debug_panel_ui,
                menu::menu_ui,
            ),
        )
            .chain(),
    );
    app.add_systems(Update, connection::send_leave);
    app.add_systems(
        PostUpdate,
        connection::unlink_session.after(LinkSystems::Send),
    );

    // Input: buffer key presses every frame, consume per fixed tick.
    add_input_and_prediction(&mut app);

    // Rendering.
    app.insert_resource(render::DebugOverlay::from_env());
    app.add_systems(Startup, (render::setup_camera, render::spawn_grid));
    app.add_systems(Update, render::apply_world_theme);
    render::add_smoothing(&mut app);
    app.add_systems(
        Update,
        (
            render::draw_arena,
            render::draw_bot_debug,
            render::draw_trails,
            render::draw_players,
            render::sync_name_labels,
            render::follow_player,
        )
            .in_set(render::RenderSet::Draw),
    );

    app.run();
}

/// The browser build draws into the page's `#bevy` canvas, sized to its parent,
/// and swallows arrow/space defaults (page scroll) so they reach the game.
fn primary_window() -> Window {
    #[cfg(target_family = "wasm")]
    {
        Window {
            canvas: Some("#bevy".into()),
            fit_canvas_to_parent: true,
            prevent_default_event_handling: true,
            ..default()
        }
    }
    #[cfg(not(target_family = "wasm"))]
    {
        Window::default()
    }
}

/// Window close button: leave the server cleanly, then exit.
fn request_quit_on_window_close(
    mut closes: MessageReader<WindowCloseRequested>,
    mut session: ResMut<connection::Session>,
    time: Res<Time<Real>>,
) {
    if closes.read().count() > 0 {
        session.request_quit(time.elapsed_secs_f64());
    }
}
