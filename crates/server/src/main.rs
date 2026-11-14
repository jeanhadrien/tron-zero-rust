//! Tron Zero — headless game server entry point.

use bevy::prelude::*;
use std::sync::Arc;
use tron_zero_server::{add_core, add_web, admission, announce, bot, ranked, rounds, systems, web};

/// Pause between main-loop iterations. Packets are read (and replies sent)
/// only once per iteration, so this bounds the polling latency added to every
/// message: 1 ms is 0.12 of a tick, in exchange for an idle cost of about 1-2%
/// of a core, where the flat-out loop burned 100%+ (measured in
/// `docs/specs/netcode-design.md`). Waiting a full tick (8.3 ms) saves nothing
/// measurable and adds up to a tick of latency each way.
const LOOP_WAIT: std::time::Duration = std::time::Duration::from_millis(1);

fn main() {
    let mut app = App::new();

    // Headless: no window. Pace the main loop (`LOOP_WAIT`); the default
    // runner spins flat out and burns a whole core doing nothing. Logging
    // goes in first: it installs the global subscriber, and every config
    // warning below would otherwise be dropped silently.
    app.add_plugins(MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(LOOP_WAIT)));
    app.add_plugins(bevy::log::LogPlugin::default());

    let mut announce_cfg = announce::AnnounceConfig::from_env();
    // Browser players: a WebTransport listener beside the UDP one. A config
    // that cannot work (e.g. WEB_PORT == PORT) stops the server; a failure to
    // load WEB_CERT/WEB_KEY disables only the web side. The room is announced
    // to browsers only once the listener is actually open (see `web`).
    let web_config = web::WebConfig::from_env().unwrap_or_else(|e| {
        tracing::error!("Invalid configuration: {e}");
        std::process::exit(2);
    });
    let web_endpoint = web_config.and_then(|cfg| match web::WebEndpoint::new(&cfg) {
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

    // A server with a ranked credential (RANKED_KEY or RANKED_IDENTITY) requires
    // join tickets and reports results.
    let ranked_state = announce_cfg
        .ranked
        .as_ref()
        .map(|_| Arc::new(ranked::RankedState::new()));

    // Detached announcer thread; handle intentionally never joined.
    let _announcer = announce::spawn_announcer(
        announce_cfg.clone(),
        Arc::clone(&counts),
        Arc::clone(&announce_handle),
        ranked_state.clone(),
    );

    if let Some(state) = ranked_state {
        app.insert_resource(ranked::RankedMode(state));
    }
    app.insert_resource(admission::MaxPlayers(announce_cfg.max_players));
    // Game mode: rounds when GAME_MODE=rounds, else the free-for-all.
    match rounds::rules_from_env(&|key| std::env::var(key).ok()) {
        Ok(Some(rules)) => {
            tracing::info!(?rules, "Rounds mode");
            app.insert_resource(rounds::MatchState::new(rules));
        }
        Ok(None) => {}
        Err(e) => {
            tracing::error!("Invalid configuration: {e}");
            std::process::exit(2);
        }
    }
    add_core(&mut app, announce_cfg.port);
    if let Some(endpoint) = web_endpoint {
        // Self-signed certificates expire within 14 days: exit before that
        // (when idle, else at the hard limit) and let the service supervisor
        // bring the server back with a new one.
        if let Some(policy) = endpoint.restart {
            app.insert_resource(policy);
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
    let roster = bot::roster_from_env(&|key| std::env::var(key).ok()).unwrap_or_else(|e| {
        tracing::error!("Invalid configuration: BOT_ROSTER: {e}");
        std::process::exit(2);
    });
    app.insert_resource(roster);
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
