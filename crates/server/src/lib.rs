//! Tron Zero — headless game server.
//!
//! Runs the authoritative simulation, replicates state to clients and executes
//! their turn commands. The library holds all gameplay wiring so integration
//! tests (`tron-zero-nettest`) run the same app as `main.rs`.

pub mod admission;
pub mod announce;
pub mod bot;
pub mod budget;
pub mod chat;
pub mod config;
pub mod elimination;
pub mod embedded;
pub mod game;
mod look;
pub mod probe;
pub mod ranked;
pub mod session;
pub mod systems;
pub mod turns;
pub mod tutorial;
pub mod web;

use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::server::*;
use lightyear::prelude::{LinkSystems, MessageSystems};
use lightyear_transport::plugin::TransportSystems;

/// Pause between main-loop iterations. Packets are read (and replies sent)
/// only once per iteration, so this bounds the polling latency added to every
/// message: 1 ms is 0.12 of a tick, in exchange for an idle cost of about 1-2%
/// of a core, where the flat-out loop burned 100%+ (measured in
/// `docs/specs/netcode-design.md`). Waiting a full tick (8.3 ms) saves nothing
/// measurable and adds up to a tick of latency each way.
const LOOP_WAIT: Duration = Duration::from_millis(1);

/// A headless app that paces its main loop (`LOOP_WAIT`; the default runner
/// spins flat out and burns a whole core doing nothing). Add logging to it
/// before [`add_game`] when the caller owns the process.
pub fn new_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(LOOP_WAIT)));
    app
}

/// The game a config describes, listening on `bind` over UDP: [`add_core`]
/// with the config's rules and player cap, and `roster` (empty: no bots) driven
/// by [`bot::add_bots`]. Shared by the server binary and the hosted server of
/// the client; each adds what only it has (announce, ranked, web, signals).
pub fn add_game(
    app: &mut App,
    config: &config::ServerConfig,
    bind: core::net::SocketAddr,
    roster: Vec<bots::config::BotBuild>,
    debug: bot::BotDebugConfig,
) -> Result<(), String> {
    let rules = config.rules()?;
    tracing::info!(?rules, "Match rules");
    app.insert_resource(admission::MaxPlayers(config.server.max_players));
    add_core(app, bind, Director::Match(rules));
    tracing::info!(roster = %config.bots.roster, bots = roster.len(), "Bot roster");
    bot::add_bots(app, roster, debug);
    Ok(())
}

/// What decides who rides when: a match of rounds, or the tutorial's steps.
pub enum Director {
    Match(shared::MatchRules),
    Tutorial(tutorial::Tutorial),
}

/// Everything a server needs except bots, the manager announcer and logging:
/// Lightyear, protocol, connection lifecycle, the `director`, respawns, turns
/// and simulation. Expects `MinimalPlugins` to be present; `bind` is the UDP
/// address to listen on. Browser players need [`add_web`] on top.
pub fn add_core(app: &mut App, bind: core::net::SocketAddr, director: Director) {
    // Required by lightyear replication internals.
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / shared::TICK_HZ),
    });
    // Register components + input type so protocol checksums match.
    shared::protocol::register_protocol(app);

    app.insert_resource(systems::UdpBind(bind));
    // New client observer — runs after RawConnectionPlugin sets up Connected + ClientOf.
    app.add_observer(systems::on_client_connected);
    // Disconnect observer — cleans up zombie player entities.
    app.add_observer(systems::on_client_disconnected);
    app.init_resource::<systems::PendingRespawns>();
    // `add_game` inserts the configured cap first; the default only serves tests.
    app.init_resource::<admission::MaxPlayers>();
    // Discovery probes are answered before the transport parses them as packets.
    app.add_systems(
        PreUpdate,
        probe::answer_probes
            .after(LinkSystems::Receive)
            .before(TransportSystems::Receive),
    );
    app.add_systems(
        PreUpdate,
        (
            systems::collect_respawn_requests,
            turns::collect_turn_commands,
            chat::collect_chat,
            session::receive_session_requests,
            // Before the hello is answered: a look sent with it is on the link
            // by the time the rider is spawned.
            look::collect_looks.before(admission::handle_hello),
            admission::handle_hello,
            admission::discard_late_handshakes.after(admission::handle_hello),
        )
            .after(MessageSystems::Receive),
    );
    app.add_systems(
        PostUpdate,
        (
            // One coalesced ack per link per frame, queued before the
            // transport serializes messages.
            turns::send_turn_acks.before(MessageSystems::Send),
            session::close_sessions.after(lightyear::prelude::LinkSystems::Send),
        ),
    );
    // Links that die without a clean leave are never despawned by Lightyear.
    // Unordered against Lightyear's own `Last` disconnect on purpose: links it
    // is closing carry `Disconnecting` and are skipped, and the despawn is a
    // `try_despawn`, so either order is safe.
    app.add_systems(Last, systems::reap_dead_links);

    // `apply_turn_commands` runs after Lightyear's `UpdateActionState`
    // (FixedPreUpdate) and overwrites its native-input fallback for human
    // riders, so only sequenced turn commands can turn a human.
    app.insert_resource(shared::SimulationRole::Server);
    // Only the server collects deaths: simulate_players re-runs on client
    // rollback, so a client must never hold a DeathLog.
    app.init_resource::<shared::DeathLog>();
    app.init_resource::<elimination::Eliminations>();
    app.add_systems(
        FixedUpdate,
        (
            systems::handle_respawns,
            turns::apply_turn_commands,
            shared::simulate_players,
            // Reads the death log before `resolve_deaths` drains it.
            game::count_scores,
            // Reads the end-of-tick state before anything is despawned.
            elimination::resolve_deaths,
            // Only ranked servers report results (see `ranked`).
            ranked::forward_eliminations.run_if(resource_exists::<ranked::RankedMode>),
        )
            .chain(),
    );
    // The director judges this tick's deaths, then places riders for the next.
    match director {
        Director::Match(rules) => {
            app.insert_resource(game::MatchState::new(rules));
            app.add_systems(
                FixedUpdate,
                game::advance_match
                    .after(elimination::resolve_deaths)
                    .before(ranked::forward_eliminations),
            );
        }
        Director::Tutorial(tutorial) => {
            app.insert_resource(tutorial);
            app.add_systems(
                FixedUpdate,
                tutorial::advance_tutorial
                    .after(elimination::resolve_deaths)
                    .before(ranked::forward_eliminations),
            );
        }
    }
    app.add_systems(
        Update,
        (
            ranked::publish_roster.run_if(resource_exists::<ranked::RankedMode>),
            systems::publish_round_trips,
        ),
    );

    // Spawn the arena once on startup (replicated to all clients).
    app.add_systems(Startup, systems::spawn_server_arena_and_start);
}

/// Also accept WebTransport sessions (browser players) on `endpoint.port`.
/// Call after [`add_core`]; the listener opens with the server at startup.
pub fn add_web(app: &mut App, endpoint: web::WebEndpoint) {
    web::add_observers(app);
    app.insert_resource(endpoint);
}
