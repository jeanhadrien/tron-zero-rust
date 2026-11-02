//! Tron Zero — headless game server entry point.

use bevy::prelude::*;
use std::sync::Arc;
use tron_zero_server::{add_core, add_web, admission, announce, bot, systems, web};

fn main() {
    let mut announce_cfg = announce::AnnounceConfig::from_env();
    // Browser players: a WebTransport listener beside the UDP one. A failure
    // here (bad WEB_CERT/WEB_KEY) disables only the web side.
    let web_endpoint =
        web::WebConfig::from_env().and_then(|cfg| match web::WebEndpoint::new(&cfg) {
            Ok(endpoint) => {
                announce_cfg.web_port = Some(cfg.advertised_port);
                announce_cfg.cert_hash = endpoint.cert_hash.clone();
                Some(endpoint)
            }
            Err(e) => {
                tracing::error!("WebTransport disabled: {e}");
                None
            }
        });
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

    // Headless: no window. Pace the main loop at the simulation rate; the
    // default runner spins flat out and burns a whole core doing nothing.
    app.add_plugins(
        MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / shared::TICK_HZ),
        )),
    );
    app.add_plugins(bevy::log::LogPlugin::default());
    app.insert_resource(admission::MaxPlayers(announce_cfg.max_players));
    add_core(&mut app, announce_cfg.port);
    if let Some(endpoint) = web_endpoint {
        // Self-signed certificates expire within 14 days: exit before that
        // and let the restart policy bring the server back with a new one.
        if let Some(uptime) = endpoint.restart_after {
            app.insert_resource(web::RestartAfter(uptime));
            app.add_systems(Update, web::exit_after_uptime);
        }
        add_web(&mut app, endpoint);
    }

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

    // Reached on a requested exit (max uptime): leave the lobby cleanly.
    announce_handle.shutdown(&announce_cfg);
}
