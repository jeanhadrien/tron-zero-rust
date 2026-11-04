//! Lobby-registry announcer: registers this server with `server-manager/`.
//!
//! NOTE: `session.rs` also talks about a "heartbeat", but that is
//! game-session liveness over UDP (idle-timeout / leave handling) —
//! unrelated to this manager heartbeat over HTTP.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, PoisonError,
    atomic::{AtomicU8, AtomicU32, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ranked::{RankedState, TicketKey};

/// How often to POST player counts while registered.
pub const HEARTBEAT_PERIOD: Duration = Duration::from_secs(2);
/// Delay between register attempts when the manager is unreachable.
pub const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Per-request HTTP timeout for all manager calls.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Manager evicts rooms idle for 40s (30s heartbeat interval + 10s grace).
/// Our 2s heartbeat / 5s retry are comfortably inside that budget.
pub const EVICTION_BUDGET: Duration = Duration::from_secs(40);

/// How long the first registration waits for the WebTransport listener to
/// report open or failed, before announcing without web fields.
const WEB_OPEN_WAIT_STEP: Duration = Duration::from_millis(100);
const WEB_OPEN_WAIT_STEPS: u32 = 50;

/// UDP port the game listener binds (also the register default).
#[derive(Resource, Debug, Clone, Copy)]
pub struct UdpPort(pub u16);

/// Handle to the live counts, written by Bevy and read by the announcer thread.
#[derive(Resource, Debug, Clone)]
pub struct PlayerCountProbe(pub Arc<LiveCounts>);

#[derive(Debug, Clone)]
pub struct AnnounceConfig {
    pub manager_url: String,
    pub advertised_host: String,
    pub port: u16,
    pub advertised_port: u16,
    pub server_name: String,
    pub max_players: u32,
    pub advertised_secure: bool,
    /// `ALLOW_LOOPBACK_ANNOUNCE`: dev opt-in that downgrades the loopback
    /// refusal (see [`AnnounceConfig::announce_blocker`]) to a warning.
    pub allow_loopback_announce: bool,
    /// UDP port of the WebTransport listener for browser players (`webPort`),
    /// set by `main` once the listener's certificate is known. `None`: no web.
    /// Only sent to the manager while the listener is actually open
    /// ([`LiveCounts::web_open`]).
    pub web_port: Option<u16>,
    /// SHA-256 of the self-signed WebTransport certificate, lowercase hex
    /// without colons (`certHash`). `None` with a CA-issued certificate.
    pub cert_hash: Option<String>,
    /// `RANKED_KEY` (`<keyId>.<secret>`): registers the room as ranked, so
    /// players need join tickets and results move ratings.
    pub ranked_key: Option<String>,
}

impl AnnounceConfig {
    /// Read config from process environment.
    pub fn from_env() -> Self {
        Self::from_map(&|key| std::env::var(key).ok())
    }

    /// Test seam: read config from any key-value lookup.
    pub fn from_map(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let manager_url = get("MANAGER_URL")
            .map(|v| clean_manager_url(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "http://localhost:3001".to_string());

        // Verbatim apart from padding: hostnames and IPs never contain
        // whitespace, and a padded value would break client resolution.
        let advertised_host = get("ADVERTISED_HOST")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let port = get("PORT")
            .filter(|v| !v.trim().is_empty())
            .map(|v| parse_port(&v))
            .unwrap_or(5000);

        // Defaults to PORT; warn and fall back on parse failure.
        let advertised_port = match get("ADVERTISED_PORT").filter(|v| !v.trim().is_empty()) {
            None => port,
            Some(raw) => match raw.trim().parse::<u16>() {
                Ok(p) => p,
                Err(_) => {
                    tracing::warn!(
                        raw = %raw,
                        fallback = port,
                        "Invalid ADVERTISED_PORT, falling back to PORT"
                    );
                    port
                }
            },
        };

        let server_name = get("SERVER_NAME")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "Unnamed Server".to_string());

        let max_players = get("MAX_PLAYERS")
            .filter(|v| !v.trim().is_empty())
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(10);

        let advertised_secure = get("ADVERTISED_SECURE").as_deref() == Some("true");

        let allow_loopback_announce = matches!(
            get("ALLOW_LOOPBACK_ANNOUNCE").as_deref().map(str::trim),
            Some("1" | "true")
        );

        Self {
            manager_url,
            advertised_host,
            port,
            advertised_port,
            server_name,
            max_players,
            advertised_secure,
            allow_loopback_announce,
            web_port: None,
            cert_hash: None,
            ranked_key: get("RANKED_KEY")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        }
    }

    pub fn register_url(&self) -> String {
        format!("{}/api/rooms", self.manager_url)
    }

    pub fn heartbeat_url(&self, room_id: &str) -> String {
        format!("{}/api/rooms/{room_id}/heartbeat", self.manager_url)
    }

    pub fn results_url(&self, room_id: &str) -> String {
        format!("{}/api/rooms/{room_id}/results", self.manager_url)
    }

    pub fn unregister_url(&self, room_id: &str) -> String {
        format!("{}/api/rooms/{room_id}", self.manager_url)
    }

    /// Why announcing would be harmful, if it would. A loopback address on a
    /// non-local manager is unreachable for every other player, and servers
    /// that all fall back to the default endpoint would fight over one room.
    /// `None` when the dev opt-in `ALLOW_LOOPBACK_ANNOUNCE` is set; see
    /// [`AnnounceConfig::announce_warning`] for the opted-in case.
    pub fn announce_blocker(&self) -> Option<String> {
        self.loopback_problem()
            .filter(|_| !self.allow_loopback_announce)
    }

    /// The loopback problem as a warning, when the opt-in overrides it.
    pub fn announce_warning(&self) -> Option<String> {
        self.loopback_problem()
            .filter(|_| self.allow_loopback_announce)
    }

    fn loopback_problem(&self) -> Option<String> {
        let loopback_host = is_loopback_host(&self.advertised_host);
        (loopback_host && !is_loopback_host(url_host(&self.manager_url))).then(|| {
            format!(
                "ADVERTISED_HOST is {} but MANAGER_URL ({}) is not local. Players could not                  reach that address, and servers sharing the default endpoint would collide.                  Set ADVERTISED_HOST (and ADVERTISED_PORT) to this server's public address",
                self.advertised_host, self.manager_url
            )
        })
    }

    /// `web_open`: the WebTransport listener is up, so browsers may be told
    /// about it. An announced-but-dead listener is a room nobody can join.
    fn register_payload(&self, web_open: bool) -> RegisterPayload {
        let (web_port, cert_hash) = if web_open {
            (self.web_port, self.cert_hash.clone())
        } else {
            (None, None)
        };
        RegisterPayload {
            host: self.advertised_host.clone(),
            port: self.advertised_port,
            // Never send explicit false; omit when insecure.
            secure: self.advertised_secure.then_some(true),
            display_name: self.server_name.clone(),
            max_players: self.max_players,
            web_port,
            cert_hash,
            ranked_key: self.ranked_key.clone(),
        }
    }
}

// Trailing slashes would double up in the URL builders; surrounding
// whitespace or quotes sneak in via copy-paste or `set VAR="..."` in cmd —
// and ureq rejects them with an opaque "invalid uri character".
fn clean_manager_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.trim_end_matches('/').to_string()
}

pub(crate) fn parse_port(raw: &str) -> u16 {
    match raw.trim().parse::<u16>() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!(raw = %raw, "Invalid PORT, falling back to 5000");
            5000
        }
    }
}

// Host part of a URL: no scheme, userinfo, port, path or IPv6 brackets.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => authority.split(':').next().unwrap_or(authority),
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

// Wire schema mirrors `server-manager/src/room.ts` — literal camelCase fields.
// No displayName tags of any kind: the manager carries Rust rooms only,
// so SERVER_NAME is sent verbatim as displayName.
#[derive(Debug, Serialize)]
pub struct RegisterPayload {
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "maxPlayers")]
    pub max_players: u32,
    /// Additive like `botCount`: managers that predate it ignore the field.
    #[serde(rename = "webPort", skip_serializing_if = "Option::is_none")]
    pub web_port: Option<u16>,
    #[serde(rename = "certHash", skip_serializing_if = "Option::is_none")]
    pub cert_hash: Option<String>,
    /// Presented once to register as ranked; the manager checks it.
    #[serde(rename = "rankedKey", skip_serializing_if = "Option::is_none")]
    pub ranked_key: Option<String>,
}

// `botCount` is additive: managers that predate it ignore the field.
#[derive(Debug, Serialize)]
pub struct HeartbeatPayload {
    #[serde(rename = "playerCount")]
    pub player_count: u32,
    #[serde(rename = "botCount")]
    pub bot_count: u32,
    /// Ranked rooms only: who is connected and the tickets not yet reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roster: Option<RosterPayload>,
}

#[derive(Debug, Serialize)]
pub struct RosterPayload {
    pub humans: Vec<String>,
    /// Base64 tickets of players admitted since this registration began.
    pub tickets: Vec<String>,
}

/// A verification key as the manager lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PublicKeyInfo {
    pub kid: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
}

/// Our room as the manager knows it: its id and the per-room secret that
/// authorizes heartbeats and the unregister.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Registration {
    pub id: String,
    pub token: String,
    /// Ranked rooms: the stable id of our ranked key (tickets are bound to it).
    #[serde(rename = "keyId", default)]
    pub key_id: Option<String>,
    #[serde(rename = "publicKeys", default)]
    pub public_keys: Vec<PublicKeyInfo>,
}

impl Registration {
    fn authorization(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

fn do_register(agent: &ureq::Agent, cfg: &AnnounceConfig, web_open: bool) -> Option<Registration> {
    match agent
        .post(&cfg.register_url())
        .send_json(cfg.register_payload(web_open))
    {
        Ok(mut res) => match res.body_mut().read_json::<Registration>() {
            Ok(reg) if !reg.token.is_empty() => Some(reg),
            Ok(_) => {
                tracing::warn!("Manager register: no room token issued");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "Manager register: bad response body");
                None
            }
        },
        Err(ureq::Error::StatusCode(409)) => {
            tracing::error!(
                host = %cfg.advertised_host,
                port = cfg.advertised_port,
                "Manager register refused (409): another room already holds this host:port. \
                 Either a previous run of this server has not expired yet (up to 40s), or two \
                 servers share ADVERTISED_HOST/ADVERTISED_PORT"
            );
            None
        }
        Err(ureq::Error::StatusCode(429)) => {
            tracing::warn!("Manager register rate-limited (429)");
            None
        }
        Err(ureq::Error::StatusCode(code)) => {
            tracing::warn!(status = code, "Manager register failed");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, url = %cfg.manager_url, "Manager unreachable");
            None
        }
    }
}

enum HeartbeatOutcome {
    Ok,
    NotFound,
    Unauthorized,
    Transient,
}

fn do_heartbeat(
    agent: &ureq::Agent,
    cfg: &AnnounceConfig,
    reg: &Registration,
    counts: &LiveCounts,
    roster: Option<RosterPayload>,
) -> HeartbeatOutcome {
    let payload = HeartbeatPayload {
        player_count: counts.humans.load(Ordering::Relaxed),
        bot_count: counts.bots.load(Ordering::Relaxed),
        roster,
    };
    let request = agent
        .post(&cfg.heartbeat_url(&reg.id))
        .header("Authorization", reg.authorization());
    match request.send_json(&payload) {
        Ok(_) => HeartbeatOutcome::Ok,
        Err(ureq::Error::StatusCode(404)) => HeartbeatOutcome::NotFound,
        Err(ureq::Error::StatusCode(401)) => {
            tracing::error!(room_id = %reg.id, "Heartbeat rejected (401): room token not accepted");
            HeartbeatOutcome::Unauthorized
        }
        Err(ureq::Error::StatusCode(code)) => {
            tracing::warn!(status = code, room_id = %reg.id, "Heartbeat failed");
            HeartbeatOutcome::Transient
        }
        Err(e) => {
            tracing::warn!(error = %e, room_id = %reg.id, "Heartbeat: manager unreachable");
            HeartbeatOutcome::Transient
        }
    }
}

/// Outcome of delivering one result batch.
enum ReportOutcome {
    /// Applied, or the manager already had it: forget the batch.
    Delivered,
    /// Manager down or the room unknown: keep the batch, resend unchanged.
    Retry,
    /// The manager refuses this batch for good (malformed): drop it.
    Rejected,
}

fn do_report(
    agent: &ureq::Agent,
    cfg: &AnnounceConfig,
    reg: &Registration,
    batch: &crate::ranked::Batch,
) -> ReportOutcome {
    let request = agent
        .post(&cfg.results_url(&reg.id))
        .header("Authorization", reg.authorization());
    match request.send_json(batch) {
        Ok(_) => ReportOutcome::Delivered,
        Err(ureq::Error::StatusCode(400)) => {
            tracing::error!(
                seq = batch.seq,
                "Manager rejected a result batch; dropping it"
            );
            ReportOutcome::Rejected
        }
        Err(e) => {
            tracing::warn!(error = %e, seq = batch.seq, "Result delivery failed; will retry");
            ReportOutcome::Retry
        }
    }
}

/// The next heartbeat's roster: current players, and the tickets of those not
/// yet reported under this registration (`sent` remembers them).
fn roster_payload(ranked: &RankedState, sent: &mut HashSet<String>) -> RosterPayload {
    let roster = ranked.roster();
    let tickets = roster
        .iter()
        .filter(|entry| !sent.contains(&entry.ticket))
        .map(|entry| entry.ticket.clone())
        .collect();
    RosterPayload {
        humans: roster.into_iter().map(|e| e.account_id).collect(),
        tickets,
    }
}

/// Best-effort blocking unregister used on shutdown.
fn unregister_blocking(cfg: &AnnounceConfig, reg: &Registration) {
    let request = agent()
        .delete(&cfg.unregister_url(&reg.id))
        .header("Authorization", reg.authorization());
    match request.call() {
        Ok(_) => tracing::info!(room_id = %reg.id, "Unregistered from manager"),
        Err(e) => {
            tracing::warn!(error = %e, room_id = %reg.id, "Failed to unregister (best-effort)")
        }
    }
}

/// Live counts written by Bevy, read by the announcer thread. Humans feed
/// `playerCount` (admission caps humans only); bots are informational.
#[derive(Debug, Default)]
pub struct LiveCounts {
    pub humans: AtomicU32,
    pub bots: AtomicU32,
    web: AtomicU8,
}

/// Lifecycle of the WebTransport listener, as the announcer needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebState {
    /// Not (yet) known to be listening: do not advertise.
    Pending = 0,
    /// Bound and accepting sessions: advertise `webPort`/`certHash`.
    Open = 1,
    /// Failed to bind or closed later: stop advertising.
    Failed = 2,
}

impl LiveCounts {
    pub fn set_web(&self, state: WebState) {
        self.web.store(state as u8, Ordering::Relaxed);
    }

    pub fn web_state(&self) -> WebState {
        match self.web.load(Ordering::Relaxed) {
            1 => WebState::Open,
            2 => WebState::Failed,
            _ => WebState::Pending,
        }
    }

    pub fn web_open(&self) -> bool {
        self.web_state() == WebState::Open
    }
}

#[derive(Debug, Default)]
struct HandleState {
    registration: Option<Registration>,
    registering: bool,
    shutting_down: bool,
}

/// State shared between the announcer thread and the shutdown handler.
/// A shutdown that lands mid-(re-)registration cannot leave the freshly
/// registered room behind: the announcer unregisters it itself and
/// `shutdown` waits for that before returning.
#[derive(Debug, Default)]
pub struct AnnounceHandle {
    state: Mutex<HandleState>,
    idle: Condvar,
}

impl AnnounceHandle {
    fn lock(&self) -> MutexGuard<'_, HandleState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// False once shutdown has begun; otherwise marks a register as in flight.
    fn begin_register(&self) -> bool {
        let mut state = self.lock();
        if state.shutting_down {
            return false;
        }
        state.registering = true;
        true
    }

    /// Records the register result. Returns false (after unregistering a room
    /// that was created too late) if shutdown began meanwhile.
    fn finish_register(&self, cfg: &AnnounceConfig, reg: Option<&Registration>) -> bool {
        let mut state = self.lock();
        if state.shutting_down {
            drop(state);
            if let Some(reg) = reg {
                unregister_blocking(cfg, reg);
            }
            self.set_idle();
            return false;
        }
        state.registration = reg.cloned();
        state.registering = false;
        self.idle.notify_all();
        true
    }

    fn set_idle(&self) {
        self.lock().registering = false;
        self.idle.notify_all();
    }

    fn clear_registration(&self) {
        self.lock().registration = None;
    }

    /// Flags shutdown and hands back the room to unregister, if any.
    fn begin_shutdown(&self) -> Option<Registration> {
        let mut state = self.lock();
        state.shutting_down = true;
        state.registration.take()
    }

    /// Shutdown hook: unregister, then wait out any in-flight register so the
    /// announcer can clean up after itself before the process exits.
    pub fn shutdown(&self, cfg: &AnnounceConfig) {
        if let Some(reg) = self.begin_shutdown() {
            unregister_blocking(cfg, &reg);
        }
        let state = self.lock();
        let _ = self
            .idle
            .wait_timeout_while(state, HTTP_TIMEOUT * 2, |s| s.registering);
    }
}

/// Spawn the announcer loop on a detached std thread (no async runtime).
/// Refuses (with a loud error) when the configuration cannot work.
pub fn spawn_announcer(
    cfg: AnnounceConfig,
    counts: Arc<LiveCounts>,
    handle: Arc<AnnounceHandle>,
    ranked: Option<Arc<RankedState>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        debug_assert!(HEARTBEAT_PERIOD * 4 < EVICTION_BUDGET);
        if let Some(problem) = cfg.announce_blocker() {
            tracing::error!("Not announcing to the manager: {problem}");
            return;
        }
        if let Some(problem) = cfg.announce_warning() {
            tracing::warn!(
                "ALLOW_LOOPBACK_ANNOUNCE is set, announcing anyway (dev only, the room will                  be listed publicly but unreachable for other players): {problem}"
            );
        }
        let agent = agent();
        // The listener opens within the first ticks; wait briefly so the first
        // registration already carries the web fields (no register/unregister
        // churn at every start). A listener that never opens is announced
        // without them.
        if cfg.web_port.is_some() {
            for _ in 0..WEB_OPEN_WAIT_STEPS {
                if counts.web_state() != WebState::Pending {
                    break;
                }
                std::thread::sleep(WEB_OPEN_WAIT_STEP);
            }
        }
        tracing::info!(url = %cfg.manager_url, "Registering with manager");
        loop {
            // Register with infinite 5s retry.
            let (reg, advertised_web) = loop {
                if !handle.begin_register() {
                    return;
                }
                let advertised_web = counts.web_open();
                let result = do_register(&agent, &cfg, advertised_web);
                if !handle.finish_register(&cfg, result.as_ref()) {
                    return;
                }
                if let Some(reg) = result {
                    break (reg, advertised_web);
                }
                tracing::warn!("Retrying register in 5s");
                std::thread::sleep(RETRY_DELAY);
            };
            tracing::info!(room_id = %reg.id, "Registered with manager");
            if let Some(ranked) = &ranked {
                match &reg.key_id {
                    Some(key_id) => {
                        let keys = reg
                            .public_keys
                            .iter()
                            .filter_map(|k| TicketKey::from_hex(&k.kid, &k.public_key))
                            .collect();
                        ranked.configure(key_id.clone(), keys);
                        tracing::info!(%key_id, "Registered as a ranked server");
                    }
                    None => tracing::error!(
                        "RANKED_KEY is set but the manager did not register us as ranked; \
                         players cannot join until it does"
                    ),
                }
            }
            // Tickets reported under this registration. Presence state is per
            // room id, so a fresh registration starts from nothing.
            let mut reported_tickets: HashSet<String> = HashSet::new();

            // Immediate heartbeat, then every 2s.
            loop {
                let roster = ranked
                    .as_ref()
                    .map(|r| roster_payload(r, &mut reported_tickets));
                let sent_tickets = roster.as_ref().map(|r| r.tickets.clone());
                match do_heartbeat(&agent, &cfg, &reg, &counts, roster) {
                    HeartbeatOutcome::Ok => {
                        reported_tickets.extend(sent_tickets.into_iter().flatten());
                    }
                    HeartbeatOutcome::Transient => {}
                    HeartbeatOutcome::NotFound => {
                        tracing::warn!("Manager lost our room — re-registering");
                        break;
                    }
                    HeartbeatOutcome::Unauthorized => {
                        // Drop the room and start over; re-registering may 409
                        // until the old entry expires, which the retry loop reports.
                        std::thread::sleep(RETRY_DELAY);
                        break;
                    }
                }
                if let Some(ranked) = &ranked
                    && let Some(batch) = ranked.next_batch()
                {
                    match do_report(&agent, &cfg, &reg, &batch) {
                        ReportOutcome::Delivered | ReportOutcome::Rejected => {
                            ranked.acknowledge(batch.seq)
                        }
                        ReportOutcome::Retry => {}
                    }
                }
                if counts.web_open() != advertised_web {
                    // The listener opened or died since we registered; the
                    // manager has no update call, so replace the room.
                    tracing::warn!(
                        web_open = counts.web_open(),
                        "WebTransport availability changed; re-registering"
                    );
                    handle.clear_registration();
                    unregister_blocking(&cfg, &reg);
                    break;
                }
                std::thread::sleep(HEARTBEAT_PERIOD);
            }
            handle.clear_registration();
        }
    })
}

/// Copy live human/bot counts into the atomics the announcer thread reads.
pub fn publish_player_count(
    probe: Res<PlayerCountProbe>,
    humans: Query<(), (With<crate::systems::Human>, With<shared::Player>)>,
    bots: Query<(), (With<crate::bot::BotBrain>, With<shared::Player>)>,
) {
    probe
        .0
        .humans
        .store(humans.iter().count() as u32, Ordering::Relaxed);
    probe
        .0
        .bots
        .store(bots.iter().count() as u32, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::HashMap;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn env_defaults_match_js() {
        let m = map(&[]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.manager_url, "http://localhost:3001");
        assert_eq!(cfg.advertised_host, "127.0.0.1");
        assert_eq!(cfg.port, 5000);
        assert_eq!(cfg.advertised_port, 5000);
        assert_eq!(cfg.server_name, "Unnamed Server");
        assert_eq!(cfg.max_players, 10);
        assert!(!cfg.advertised_secure);
        assert_eq!(cfg.register_url(), "http://localhost:3001/api/rooms");
    }

    #[test]
    fn full_overrides_trim_and_parse() {
        let m = map(&[
            ("MANAGER_URL", "http://manager:3001///"),
            ("ADVERTISED_HOST", "10.0.0.5"),
            ("PORT", "5001"),
            ("ADVERTISED_PORT", "5002"),
            ("SERVER_NAME", "Rust Arena"),
            ("MAX_PLAYERS", "16"),
            ("ADVERTISED_SECURE", "true"),
        ]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.manager_url, "http://manager:3001");
        assert_eq!(cfg.advertised_host, "10.0.0.5");
        assert_eq!(cfg.port, 5001);
        assert_eq!(cfg.advertised_port, 5002);
        assert_eq!(cfg.server_name, "Rust Arena");
        assert_eq!(cfg.max_players, 16);
        assert!(cfg.advertised_secure);
        assert_eq!(
            cfg.heartbeat_url("abc"),
            "http://manager:3001/api/rooms/abc/heartbeat"
        );
        assert_eq!(
            cfg.unregister_url("abc"),
            "http://manager:3001/api/rooms/abc"
        );
    }

    #[test]
    fn whitespace_server_name_falls_back_to_default() {
        let m = map(&[("SERVER_NAME", "   ")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.server_name, "Unnamed Server");
    }

    #[test]
    fn advertised_host_is_trimmed() {
        // Padding would otherwise be advertised verbatim and break client
        // resolution ("127.0.0.1 " misses the IP-literal fast path).
        let m = map(&[("ADVERTISED_HOST", "  10.0.0.5 ")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.advertised_host, "10.0.0.5");
        let m = map(&[("ADVERTISED_HOST", "   ")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.advertised_host, "127.0.0.1");
    }

    #[test]
    fn manager_url_cleans_whitespace_and_cmd_quotes() {
        // Copy-paste padding and cmd's `set VAR="..."` quote embedding both
        // produce ureq's opaque "invalid uri character" without cleaning.
        let m = map(&[("MANAGER_URL", "  https://example.com:3001/ ")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.register_url(), "https://example.com:3001/api/rooms");
        let m = map(&[("MANAGER_URL", "\"https://example.com:3001/\"")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.register_url(), "https://example.com:3001/api/rooms");
    }

    #[test]
    fn advertised_port_falls_back_to_port() {
        // Unset -> PORT.
        let m = map(&[("PORT", "5007")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.advertised_port, 5007);
        // Invalid -> PORT with a warning.
        let m = map(&[("PORT", "5007"), ("ADVERTISED_PORT", "bogus")]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert_eq!(cfg.advertised_port, 5007);
    }

    #[test]
    fn register_payload_key_set_matches_room_ts() {
        let cfg = AnnounceConfig {
            manager_url: "http://localhost:3001".into(),
            advertised_host: "127.0.0.1".into(),
            port: 5000,
            advertised_port: 5000,
            server_name: "Rust Arena".into(),
            max_players: 10,
            advertised_secure: false,
            allow_loopback_announce: false,
            web_port: None,
            cert_hash: None,
            ranked_key: None,
        };
        let value = serde_json::to_value(cfg.register_payload(false)).unwrap();
        let obj = value.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 4);
        assert!(obj.contains_key("host"));
        assert!(obj.contains_key("port"));
        assert!(obj.contains_key("displayName"));
        assert!(obj.contains_key("maxPlayers"));
        assert!(!obj.contains_key("secure"), "secure omitted when None");
        assert!(!obj.contains_key("displayname"));
        assert!(!obj.contains_key("display_name"));
        assert!(!obj.contains_key("max_players"));
        assert_eq!(obj["displayName"], Value::String("Rust Arena".into()));

        // secure=true serializes; never an explicit false.
        let secure_cfg = AnnounceConfig {
            advertised_secure: true,
            ..cfg.clone()
        };
        let secure_value = serde_json::to_value(secure_cfg.register_payload(false)).unwrap();
        assert_eq!(secure_value["secure"], Value::Bool(true));

        // The web fields appear only when the WebTransport listener is on.
        let web_cfg = AnnounceConfig {
            web_port: Some(5001),
            cert_hash: Some("ab".repeat(32)),
            ..cfg
        };
        let web_value = serde_json::to_value(web_cfg.register_payload(true)).unwrap();
        assert_eq!(web_value["webPort"], Value::Number(5001.into()));
        assert_eq!(web_value["certHash"], Value::String("ab".repeat(32)));

        // ...and only while the listener is actually open.
        let closed_value = serde_json::to_value(web_cfg.register_payload(false)).unwrap();
        assert!(closed_value.get("webPort").is_none());
        assert!(closed_value.get("certHash").is_none());
    }

    #[test]
    fn periods_fit_comfortably_inside_40s_eviction() {
        assert_eq!(HEARTBEAT_PERIOD, Duration::from_secs(2));
        assert_eq!(RETRY_DELAY, Duration::from_secs(5));
        assert_eq!(EVICTION_BUDGET, Duration::from_secs(40));
        assert!(HEARTBEAT_PERIOD * 4 < EVICTION_BUDGET);
        assert!(RETRY_DELAY * 2 < EVICTION_BUDGET);
        assert!(HTTP_TIMEOUT <= RETRY_DELAY);
    }

    #[test]
    fn heartbeat_payload_is_humans_and_bots() {
        let payload = HeartbeatPayload {
            player_count: 3,
            bot_count: 5,
            roster: None,
        };
        let value = serde_json::to_value(payload).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.len(), 2);
        assert_eq!(obj["playerCount"], Value::Number(3.into()));
        assert_eq!(obj["botCount"], Value::Number(5.into()));
    }

    fn registration(body: &str) -> Registration {
        serde_json::from_str::<Registration>(body).unwrap()
    }

    #[test]
    fn register_response_with_token_authorizes_as_bearer() {
        let reg = registration(r#"{"id":"abc","host":"h","port":1,"token":"s3cret"}"#);
        assert_eq!(reg.id, "abc");
        assert_eq!(reg.authorization(), "Bearer s3cret");
    }

    #[test]
    fn register_response_without_token_is_rejected() {
        assert!(serde_json::from_str::<Registration>(r#"{"id":"abc","port":1}"#).is_err());
    }

    fn cfg_for(manager: &str, host: &str) -> AnnounceConfig {
        let m = map(&[("MANAGER_URL", manager), ("ADVERTISED_HOST", host)]);
        AnnounceConfig::from_map(&|k| m.get(k).cloned())
    }

    #[test]
    fn loopback_host_with_remote_manager_is_blocked() {
        let remote = "https://manager-abc.run.app";
        assert!(cfg_for(remote, "127.0.0.1").announce_blocker().is_some());
        assert!(cfg_for(remote, "localhost").announce_blocker().is_some());
        assert!(cfg_for(remote, "0.0.0.0").announce_blocker().is_some());
        assert!(cfg_for(remote, "::1").announce_blocker().is_some());
        // A real address, or a local manager, is fine.
        assert!(
            cfg_for(remote, "play.example.com")
                .announce_blocker()
                .is_none()
        );
        assert!(cfg_for(remote, "203.0.113.7").announce_blocker().is_none());
        assert!(
            cfg_for("http://localhost:3001", "127.0.0.1")
                .announce_blocker()
                .is_none()
        );
        assert!(
            cfg_for("http://[::1]:3001/x", "127.0.0.1")
                .announce_blocker()
                .is_none()
        );
        assert!(
            cfg_for("http://user@127.0.0.1:3001", "127.0.0.1")
                .announce_blocker()
                .is_none()
        );
    }

    #[test]
    fn loopback_announce_opt_in_turns_the_block_into_a_warning() {
        let remote = "https://manager-abc.run.app";
        let m = map(&[
            ("MANAGER_URL", remote),
            ("ADVERTISED_HOST", "127.0.0.1"),
            ("ALLOW_LOOPBACK_ANNOUNCE", "1"),
        ]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert!(cfg.allow_loopback_announce);
        assert!(cfg.announce_blocker().is_none());
        assert!(cfg.announce_warning().is_some());
        // Off by default, and anything but 1/true does not count.
        assert!(!cfg_for(remote, "127.0.0.1").allow_loopback_announce);
        let m = map(&[("ALLOW_LOOPBACK_ANNOUNCE", "0")]);
        assert!(!AnnounceConfig::from_map(&|k| m.get(k).cloned()).allow_loopback_announce);
        // Nothing to warn about when the setup is fine anyway.
        let m = map(&[
            ("MANAGER_URL", remote),
            ("ADVERTISED_HOST", "play.example.com"),
            ("ALLOW_LOOPBACK_ANNOUNCE", "1"),
        ]);
        let cfg = AnnounceConfig::from_map(&|k| m.get(k).cloned());
        assert!(cfg.announce_warning().is_none());
    }

    #[test]
    fn url_host_strips_scheme_port_path_and_userinfo() {
        assert_eq!(url_host("https://a.b.c:8443/x?y#z"), "a.b.c");
        assert_eq!(url_host("http://u:p@host:1"), "host");
        assert_eq!(url_host("http://[::1]:3001"), "::1");
        assert_eq!(url_host("localhost:3001"), "localhost");
    }

    #[test]
    fn shutdown_takes_registration_and_stops_further_registers() {
        let handle = AnnounceHandle::default();
        let cfg = cfg_for("http://localhost:3001", "127.0.0.1");
        let reg = Registration {
            id: "r1".into(),
            token: "t".into(),
            key_id: None,
            public_keys: Vec::new(),
        };
        assert!(handle.begin_register());
        assert!(handle.finish_register(&cfg, Some(&reg)));
        assert_eq!(handle.begin_shutdown(), Some(reg));
        assert!(!handle.begin_register());
    }

    #[test]
    fn shutdown_during_register_aborts_and_releases_the_waiter() {
        let handle = AnnounceHandle::default();
        let cfg = cfg_for("http://localhost:3001", "127.0.0.1");
        assert!(handle.begin_register());
        // Shutdown lands while the register request is in flight...
        assert_eq!(handle.begin_shutdown(), None);
        assert!(handle.lock().registering);
        // ...so the late result is refused and the in-flight flag is cleared
        // (a Some(reg) here would be unregistered by the announcer itself).
        assert!(!handle.finish_register(&cfg, None));
        assert!(!handle.lock().registering);
    }
}
