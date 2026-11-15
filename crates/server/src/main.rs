//! Tron Zero — headless game server entry point.

use bevy::prelude::*;
use std::sync::Arc;
use tron_zero_server::{add_game, add_web, announce, bot, config, new_app, ranked, web};

fn main() {
    let mut app = new_app();

    // Logging goes in first: it installs the global subscriber, and every
    // config warning below would otherwise be dropped silently.
    app.add_plugins(bevy::log::LogPlugin::default());

    // Game settings from the config file (SERVER_CONFIG, else the official
    // one); deployment settings from env vars. A bad file stops the server.
    let config = config::ServerConfig::from_env().unwrap_or_else(|e| {
        tracing::error!("Invalid configuration: {e}");
        std::process::exit(2);
    });
    let mut announce_cfg = announce::AnnounceConfig::from_env(&config.server);
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
    let roster = config.roster().unwrap_or_else(|e| {
        tracing::error!("Invalid configuration: bots.roster: {e}");
        std::process::exit(2);
    });
    // The bot debug gate is read once from the environment.
    let debug = bot::BotDebugConfig::from_env();
    let bind = ([0, 0, 0, 0], announce_cfg.port).into();
    add_game(&mut app, &config, bind, roster, debug).unwrap_or_else(|e| {
        tracing::error!("Invalid configuration: {e}");
        std::process::exit(2);
    });
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

    app.run();

    // Reached on a requested exit (max uptime): leave the lobby cleanly.
    announce_handle.shutdown(&announce_cfg);
}
