//! Lobby-registry announcer: registers this server with the JS manager.
//!
//! NOTE: `session.rs` also talks about a "heartbeat", but that is
//! game-session liveness over UDP (idle-timeout / leave handling) —
//! unrelated to this manager heartbeat over HTTP.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

/// How often to POST player counts while registered.
pub const HEARTBEAT_PERIOD: Duration = Duration::from_secs(2);
/// Delay between register attempts when the manager is unreachable.
pub const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Per-request HTTP timeout for all manager calls.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Manager evicts rooms idle for 40s (30s heartbeat interval + 10s grace).
/// Our 2s heartbeat / 5s retry are comfortably inside that budget.
pub const EVICTION_BUDGET: Duration = Duration::from_secs(40);

/// UDP port the game listener binds (also the register default).
#[derive(Resource, Debug, Clone, Copy)]
pub struct UdpPort(pub u16);

/// Shared player count written by Bevy, read by the announcer thread.
#[derive(Resource, Debug, Clone)]
pub struct PlayerCountProbe(pub Arc<AtomicU32>);

#[derive(Debug, Clone)]
pub struct AnnounceConfig {
    pub manager_url: String,
    pub advertised_host: String,
    pub port: u16,
    pub advertised_port: u16,
    pub server_name: String,
    pub max_players: u32,
    pub advertised_secure: bool,
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

        Self {
            manager_url,
            advertised_host,
            port,
            advertised_port,
            server_name,
            max_players,
            advertised_secure,
        }
    }

    pub fn register_url(&self) -> String {
        format!("{}/api/rooms", self.manager_url)
    }

    pub fn heartbeat_url(&self, room_id: &str) -> String {
        format!("{}/api/rooms/{room_id}/heartbeat", self.manager_url)
    }

    pub fn unregister_url(&self, room_id: &str) -> String {
        format!("{}/api/rooms/{room_id}", self.manager_url)
    }

    fn register_payload(&self) -> RegisterPayload {
        RegisterPayload {
            host: self.advertised_host.clone(),
            port: self.advertised_port,
            // Never send explicit false; omit when insecure.
            secure: self.advertised_secure.then_some(true),
            display_name: self.server_name.clone(),
            max_players: self.max_players,
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

fn parse_port(raw: &str) -> u16 {
    match raw.trim().parse::<u16>() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!(raw = %raw, "Invalid PORT, falling back to 5000");
            5000
        }
    }
}

// Wire schema mirrors JS `RegisterPayload` — literal camelCase fields.
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
}

#[derive(Debug, Serialize)]
pub struct HeartbeatPayload {
    #[serde(rename = "playerCount")]
    pub player_count: u32,
}

#[derive(Debug, Deserialize)]
struct RegisterResponse {
    id: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

fn do_register(agent: &ureq::Agent, cfg: &AnnounceConfig) -> Option<String> {
    match agent
        .post(&cfg.register_url())
        .send_json(cfg.register_payload())
    {
        Ok(mut res) => match res.body_mut().read_json::<RegisterResponse>() {
            Ok(body) => Some(body.id),
            Err(e) => {
                tracing::warn!(error = %e, "Manager register: bad response body");
                None
            }
        },
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
    Transient,
}

fn do_heartbeat(
    agent: &ureq::Agent,
    cfg: &AnnounceConfig,
    room_id: &str,
    player_count: u32,
) -> HeartbeatOutcome {
    let payload = HeartbeatPayload { player_count };
    match agent.post(&cfg.heartbeat_url(room_id)).send_json(&payload) {
        Ok(_) => HeartbeatOutcome::Ok,
        Err(ureq::Error::StatusCode(404)) => HeartbeatOutcome::NotFound,
        Err(ureq::Error::StatusCode(code)) => {
            tracing::warn!(status = code, room_id = %room_id, "Heartbeat failed");
            HeartbeatOutcome::Transient
        }
        Err(e) => {
            tracing::warn!(error = %e, room_id = %room_id, "Heartbeat: manager unreachable");
            HeartbeatOutcome::Transient
        }
    }
}

/// Best-effort blocking unregister used by the shutdown handler.
pub fn unregister_blocking(cfg: &AnnounceConfig, room_id: &str) {
    match agent().delete(&cfg.unregister_url(room_id)).call() {
        Ok(_) => tracing::info!(room_id = %room_id, "Unregistered from manager"),
        Err(e) => {
            tracing::warn!(error = %e, room_id = %room_id, "Failed to unregister (best-effort)")
        }
    }
}

/// Spawn the announcer loop on a detached std thread (no async runtime).
/// The room id lives in the thread; it is also mirrored into `room_id_slot`
/// (set on register, cleared on 404) for the shutdown handler.
pub fn spawn_announcer(
    cfg: AnnounceConfig,
    player_count: Arc<AtomicU32>,
    room_id_slot: Arc<Mutex<Option<String>>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        debug_assert!(HEARTBEAT_PERIOD * 4 < EVICTION_BUDGET);
        let agent = agent();
        tracing::info!(url = %cfg.manager_url, "Registering with manager");
        loop {
            // Register with infinite 5s retry.
            let room_id = loop {
                if let Some(id) = do_register(&agent, &cfg) {
                    break id;
                }
                tracing::warn!("Retrying register in 5s");
                std::thread::sleep(RETRY_DELAY);
            };
            tracing::info!(room_id = %room_id, "Registered with manager");
            if let Ok(mut slot) = room_id_slot.lock() {
                *slot = Some(room_id.clone());
            }

            // Immediate heartbeat, then every 2s.
            let count = player_count.load(Ordering::Relaxed);
            if matches!(
                do_heartbeat(&agent, &cfg, &room_id, count),
                HeartbeatOutcome::NotFound
            ) {
                tracing::warn!("Manager lost our room — re-registering");
                if let Ok(mut slot) = room_id_slot.lock() {
                    *slot = None;
                }
                continue;
            }
            loop {
                std::thread::sleep(HEARTBEAT_PERIOD);
                let count = player_count.load(Ordering::Relaxed);
                match do_heartbeat(&agent, &cfg, &room_id, count) {
                    HeartbeatOutcome::Ok => {}
                    HeartbeatOutcome::Transient => {} // keep id, warn already logged
                    HeartbeatOutcome::NotFound => {
                        tracing::warn!("Manager lost our room — re-registering");
                        if let Ok(mut slot) = room_id_slot.lock() {
                            *slot = None;
                        }
                        break; // back to outer register loop
                    }
                }
            }
        }
    })
}

/// Copy the live player count into the atomic the announcer thread reads.
pub fn publish_player_count(
    probe: Res<PlayerCountProbe>,
    humans: Query<(), (With<crate::systems::Human>, With<shared::Player>)>,
    bots: Query<&shared::IsAlive, (With<crate::bot::BotBrain>, With<shared::Player>)>,
) {
    let human_count = humans.iter().count();
    // Bots count alive-only. Deliberate divergence from JS, which counts dead
    // bots too (they despawn after 2s and shouldn't inflate the browser count).
    let bot_count = bots.iter().filter(|alive| alive.0).count();
    probe
        .0
        .store((human_count + bot_count) as u32, Ordering::Relaxed);
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
        };
        let value = serde_json::to_value(cfg.register_payload()).unwrap();
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
            ..cfg
        };
        let secure_value = serde_json::to_value(secure_cfg.register_payload()).unwrap();
        assert_eq!(secure_value["secure"], Value::Bool(true));
    }

    #[test]
    fn heartbeat_payload_is_exactly_player_count() {
        let value = serde_json::to_value(HeartbeatPayload { player_count: 3 }).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj["playerCount"], Value::Number(3.into()));
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
}
