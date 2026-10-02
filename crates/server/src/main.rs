//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients,
//! and processes client inputs via lightyear.

mod announce;
mod bot;
mod session;
mod systems;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::server::*;
use std::sync::{Arc, Mutex, atomic::AtomicU32};

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
    // Required by lightyear replication internals.
    app.add_plugins(bevy::state::app::StatesPlugin);

    // --- Lightyear ---
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });

    // Register components + input type so protocol checksums match.
    shared::protocol::register_protocol(&mut app);

    // UDP bind port comes from env (PORT); startup bind reads it.
    app.insert_resource(announce::UdpPort(announce_cfg.port));
    // Player count published for the announcer heartbeat.
    app.insert_resource(announce::PlayerCountProbe(player_count));
    app.add_systems(Update, announce::publish_player_count);

    // New client observer — runs after RawConnectionPlugin sets up Connected + ClientOf.
    app.add_observer(systems::on_client_connected);
    // Disconnect observer — cleans up zombie player entities.
    app.add_observer(systems::on_client_disconnected);
    app.init_resource::<systems::PendingRespawns>();
    app.add_systems(
        PreUpdate,
        systems::collect_respawn_requests.after(lightyear::prelude::MessageSystems::Receive),
    );
    app.add_systems(
        PreUpdate,
        session::receive_session_requests.after(lightyear::prelude::MessageSystems::Receive),
    );
    app.add_systems(
        PostUpdate,
        session::close_sessions.after(lightyear::prelude::LinkSystems::Send),
    );

    // Simulation systems in FixedUpdate.
    app.insert_resource(shared::SimulationRole::Server);
    app.add_systems(
        FixedUpdate,
        (
            systems::handle_respawns,
            bot::bot_brain_input,
            shared::simulate_players,
            bot::bot_spawner,
        )
            .chain(),
    );

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(
        Startup,
        (systems::spawn_server_arena_and_start, bot::setup_bots).chain(),
    );

    app.run();
}
