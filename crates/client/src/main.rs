//! Tron Zero — client entry point.
//!
//! Connects to a lightyear server via UDP / raw connection, sends keyboard
//! inputs, predicts the local lightcycle, and renders the arena + players.

use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use lightyear::prelude::*;
use tron_zero_client::{
    add_input_and_prediction, add_network, attach_local_input, browser, connection, input, menu,
    render, settings,
};

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins);
    app.add_plugins(EguiPlugin::default());
    add_network(&mut app);

    app.init_resource::<connection::Session>();
    app.init_resource::<menu::MenuState>();
    app.insert_resource(browser::ManagerConfig::from_env());
    app.init_resource::<browser::BrowserState>();
    app.add_systems(Startup, browser::initial_refresh);
    app.add_systems(PreUpdate, browser::poll_browser_fetch);
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
        (
            render::hud_ui,
            render::death_ui,
            render::debug_panel_ui,
            menu::menu_ui,
        ),
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
    app.add_systems(Startup, render::setup_camera);
    app.add_systems(
        Update,
        (
            render::draw_arena,
            render::draw_bot_debug,
            render::draw_trails,
            render::draw_players,
            render::sync_name_labels,
            render::follow_player,
        ),
    );

    app.run();
}
