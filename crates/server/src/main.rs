//! Tron Zero — headless game server entry point.

use bevy::prelude::*;
use std::sync::Arc;
use tron_zero_server::{add_core, admission, announce, bot, systems};

fn main() {
    let announce_cfg = announce::AnnounceConfig::from_env();
    let counts = Arc::new(announce::LiveCounts::default());
    let announce_handle = Arc::new(announce::AnnounceHandle::default());

    // Best-effort unregister on SIGINT/SIGTERM/SIGHUP. The manager's 40s
    // eviction (stale-room sweep) is the backstop if we crash before this runs.
    {
        let cfg = announce_cfg.clone();
        let handle = Arc::clone(&announce_handle);
        let _ = ctrlc::set_handler(move || {
            handle.shutdown(&cfg);
            std::process::exit(0);
        });
    }

    // Detached announcer thread; handle intentionally never joined.
    let _announcer = announce::spawn_announcer(
        announce_cfg.clone(),
        Arc::clone(&counts),
        Arc::clone(&announce_handle),
    );

    let mut app = App::new();

    // Headless: no window, event-driven I/O loop.
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    app.insert_resource(admission::MaxPlayers(announce_cfg.max_players));
    add_core(&mut app, announce_cfg.port);

    // Human/bot counts published for the announcer heartbeat.
    app.insert_resource(announce::PlayerCountProbe(counts));
    app.add_systems(Update, announce::publish_player_count);

    // Bots: staggered decisions (one epoch per reaction period). The brain
    // writes input after respawns and before the shared simulation; spawning
    // follows it. The debug gate is read once from the environment.
    app.init_resource::<bot::SchedulerConfig>();
    app.insert_resource(bot::BotDebugConfig::from_env());
    app.add_systems(
        FixedUpdate,
        (
            bot::bot_brain_input
                .after(systems::handle_respawns)
                .before(shared::simulate_players),
            bot::bot_spawner.after(shared::simulate_players),
        ),
    );
    app.add_systems(
        Startup,
        bot::setup_bots.after(systems::spawn_server_arena_and_start),
    );

    app.run();
}
