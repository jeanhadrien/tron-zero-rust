//! Tron Zero — headless game server entry point.

use bevy::prelude::*;
use std::sync::{Arc, Mutex, atomic::AtomicU32};
use tron_zero_server::{add_core, announce, bot, systems};

fn main() {
    let announce_cfg = announce::AnnounceConfig::from_env();
    let player_count = Arc::new(AtomicU32::new(0));
    let room_id_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Best-effort unregister on shutdown. The manager's 40s eviction
    // (stale-room sweep) is the backstop if we crash before this runs.
    {
        let cfg = announce_cfg.clone();
        let slot = Arc::clone(&room_id_slot);
        let _ = ctrlc::set_handler(move || {
            let id = slot.lock().ok().and_then(|guard| guard.clone());
            if let Some(id) = id {
                announce::unregister_blocking(&cfg, &id);
            }
            std::process::exit(0);
        });
    }

    // Detached announcer thread; handle intentionally never joined.
    let _announcer = announce::spawn_announcer(
        announce_cfg.clone(),
        Arc::clone(&player_count),
        Arc::clone(&room_id_slot),
    );

    let mut app = App::new();

    // Headless: no window, event-driven I/O loop.
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    add_core(&mut app, announce_cfg.port);

    // Player count published for the announcer heartbeat.
    app.insert_resource(announce::PlayerCountProbe(player_count));
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
