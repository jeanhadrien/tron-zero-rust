//! Server browser: room list from `server-manager/` plus rows for servers
//! detected on this machine.
//!
//! Local rows come from `local_scan` (a probe answered by the server), so a
//! localhost entry exists only while a server is actually there. Manager rows
//! are never probed; the manager's heartbeats already prove liveness, and stale
//! entries sink via the `last_heartbeat` sort. A manager room that points at a
//! detected local server is hidden so the local row wins (`merge_rows`).
//!
//! Fetch uses `ehttp` (egui's fetch lib): one API on native and wasm, no
//! async runtime, no threads or channels of our own. `request_refresh` fires
//! a request unless one is already in flight; `poll_browser_fetch` drains the
//! completed result once per frame. DNS resolution is a blocking
//! `ToSocketAddrs` call, but it runs once per refresh on receipt — never per
//! frame — so no worker thread is warranted.
//!
//! Native clients dial a room's UDP `port`. The browser can only speak
//! WebTransport: it dials `webPort` and pins the room's `certHash`, and it
//! cannot resolve hostnames (lightyear dials `https://<ip>:<port>`), so a
//! web row is joinable only when it announces an IP, a `webPort` and a hash.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use serde::Deserialize;

use crate::connection::Endpoint;
use crate::local_scan::{LocalServer, LocalServers};

const LOCAL_WEB_PORT: u16 = 5001;

/// Whether this build dials rooms over WebTransport (the browser) rather than UDP.
const WEB: bool = cfg!(target_family = "wasm");

// One room as served by `GET /api/rooms` (see `server-manager/src/room.ts`).
// `secure` is deliberately absent: serde drops unknown fields, so omitting
// it makes accidental use of the geckos-only flag unrepresentable.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInfo {
    pub id: String,
    pub host: String,
    pub port: u16,
    // WebTransport (QUIC) UDP port; absent on native-only rooms.
    #[serde(default)]
    pub web_port: Option<u16>,
    // Hex SHA-256 of the room's self-signed certificate; absent when the
    // server uses a CA certificate.
    #[serde(default)]
    pub cert_hash: Option<String>,
    pub display_name: String,
    // Humans only; `max_players` caps humans too, so Full compares the two.
    pub player_count: u32,
    pub max_players: u32,
    // Informational (bots never count against capacity); absent on older managers.
    #[serde(default)]
    pub bot_count: u32,
    #[serde(default)]
    pub last_heartbeat: u64,
    // Ranked rooms need a manager-signed join ticket; absent on older managers.
    #[serde(default)]
    pub ranked: bool,
}

impl RoomInfo {
    // `max_players == 0` means "unknown": never full.
    pub fn is_full(&self) -> bool {
        self.max_players > 0 && self.player_count >= self.max_players
    }
}

// One rendered row: the room plus where to dial it, or why it can't be dialed.
#[derive(Debug, Clone)]
pub struct BrowserRow {
    pub room: RoomInfo,
    pub endpoint: Option<Endpoint>,
    pub problem: Option<String>,
    pub is_local: bool,
}

impl BrowserRow {
    // The port this build dials (the WebTransport one in the browser).
    pub fn play_port(&self) -> u16 {
        match &self.endpoint {
            Some(endpoint) => endpoint.addr.port(),
            None if WEB => self.room.web_port.unwrap_or(self.room.port),
            None => self.room.port,
        }
    }
}

// Rows for servers on this machine. Native builds list what `local_scan`
// found. The browser cannot probe, so it offers the dev server on the local
// WebTransport port only when the page is served from a local dev host with the
// server's certificate digest in `?TRON_CERT_DIGEST=` (the server logs it at
// startup); `env_var` yields nothing elsewhere.
pub fn local_rows(servers: &LocalServers) -> Vec<BrowserRow> {
    if WEB {
        let digest = crate::platform::env_var("TRON_CERT_DIGEST").filter(|d| !d.trim().is_empty());
        return digest.into_iter().map(web_local_row).collect();
    }
    servers.found().iter().map(native_local_row).collect()
}

fn local_room(port: u16, web_port: Option<u16>, cert_hash: Option<String>) -> RoomInfo {
    RoomInfo {
        id: format!("local-{port}"),
        host: Ipv4Addr::LOCALHOST.to_string(),
        port,
        web_port,
        cert_hash,
        display_name: "Localhost".into(),
        player_count: 0,
        max_players: 0,
        bot_count: 0,
        last_heartbeat: 0,
        ranked: false,
    }
}

fn native_local_row(server: &LocalServer) -> BrowserRow {
    let mut room = local_room(server.port, None, None);
    room.player_count = u32::from(server.reply.players);
    room.max_players = u32::from(server.reply.max_players);
    let compatible = server.reply.compatible();
    BrowserRow {
        room,
        endpoint: compatible.then(|| server.addr().into()),
        problem: (!compatible).then(|| "Incompatible game version: update the client.".to_owned()),
        is_local: true,
    }
}

fn web_local_row(digest: String) -> BrowserRow {
    BrowserRow {
        room: local_room(5000, Some(LOCAL_WEB_PORT), Some(digest.clone())),
        endpoint: Some(Endpoint {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, LOCAL_WEB_PORT)),
            cert_digest: Some(digest),
        }),
        problem: None,
        is_local: true,
    }
}

// Local rows first, then manager rows; a manager room that resolves to a
// detected local server is dropped so the local row wins and never duplicates.
pub fn merge_rows(local: Vec<BrowserRow>, manager: &[BrowserRow]) -> Vec<BrowserRow> {
    let local_addrs: Vec<SocketAddr> = local
        .iter()
        .filter_map(|row| row.endpoint.as_ref().map(|endpoint| endpoint.addr))
        .collect();
    let mut rows = local;
    rows.extend(
        manager
            .iter()
            .filter(|row| {
                row.endpoint
                    .as_ref()
                    .is_none_or(|endpoint| !local_addrs.contains(&endpoint.addr))
            })
            .cloned(),
    );
    rows
}

// Trailing slashes would double up in rooms_url(); surrounding whitespace
// or quotes sneak in via copy-paste or `set VAR="..."` in cmd — and ureq
// rejects them with an opaque "invalid uri character". Clean once here.
fn clean_base_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted.trim_end_matches('/').to_owned()
}

// Busiest rooms first, freshest heartbeat breaks ties.
pub fn sort_manager_rows(mut rooms: Vec<RoomInfo>) -> Vec<RoomInfo> {
    rooms.sort_by(|a, b| {
        b.player_count
            .cmp(&a.player_count)
            .then(b.last_heartbeat.cmp(&a.last_heartbeat))
    });
    rooms
}

// Parse the manager's room array.
// Hosts are trimmed: a padded "127.0.0.1 " would otherwise miss the IP-literal
// fast path, hit DNS, and fail — for data, display, and dedup alike.
pub fn parse_rooms(body: &[u8]) -> Result<Vec<RoomInfo>, String> {
    let mut rooms: Vec<RoomInfo> = serde_json::from_slice(body)
        .map_err(|err| format!("Could not parse manager response: {err}"))?;
    for room in &mut rooms {
        room.host = room.host.trim().to_owned();
    }
    Ok(rooms)
}

// Wording shown in the browser when the manager answers with an error status.
pub fn http_status_error(status: u16) -> String {
    format!("Manager returned {status}")
}

pub fn transport_error(url: &str, err: impl std::fmt::Display) -> String {
    format!("Could not reach manager at {url}: {err}")
}

// Where to dial a room, or why it can't be dialed. Numeric IPs never touch
// DNS, so this is offline-safe for them; hostnames may block briefly natively,
// which is why this runs once per refresh on receipt, not per frame.
fn resolve_room(room: &RoomInfo) -> (Option<Endpoint>, Option<String>) {
    if WEB {
        web_endpoint(room)
    } else {
        native_endpoint(room)
    }
}

fn native_endpoint(room: &RoomInfo) -> (Option<Endpoint>, Option<String>) {
    use std::net::ToSocketAddrs as _;
    match (room.host.as_str(), room.port).to_socket_addrs() {
        Ok(addrs) => {
            let mut addrs: Vec<SocketAddr> = addrs.collect();
            // Windows may resolve "localhost" to ::1 first; the server binds IPv4.
            addrs.sort_by_key(|addr| if addr.is_ipv4() { 0 } else { 1 });
            match addrs.into_iter().next() {
                Some(addr) => (Some(addr.into()), None),
                None => (
                    None,
                    Some(format!(
                        "Could not resolve {}: no addresses found",
                        room.host
                    )),
                ),
            }
        }
        Err(err) => (
            None,
            Some(format!("Could not resolve {}: {err}", room.host)),
        ),
    }
}

// Browsers dial `host:webPort` pinning the room's certificate hash.
fn web_endpoint(room: &RoomInfo) -> (Option<Endpoint>, Option<String>) {
    let Some(web_port) = room.web_port else {
        return (None, Some("Desktop only: no browser support.".into()));
    };
    let Some(hash) = room.cert_hash.as_ref().filter(|hash| !hash.is_empty()) else {
        return (
            None,
            Some("Not joinable from a browser: no certificate hash announced.".into()),
        );
    };
    let Ok(ip) = room.host.parse::<IpAddr>() else {
        return (
            None,
            Some(format!(
                "Not joinable from a browser: {} is a hostname, not an IP address.",
                room.host
            )),
        );
    };
    (
        Some(Endpoint {
            addr: SocketAddr::new(ip, web_port),
            cert_digest: Some(hash.to_ascii_lowercase()),
        }),
        None,
    )
}

// Manager rooms as rows, busiest first. The browser lists only rooms that
// announce a `webPort`.
fn build_rows(rooms: Vec<RoomInfo>) -> Vec<BrowserRow> {
    sort_manager_rows(rooms)
        .into_iter()
        .filter(|room| !(WEB && room.web_port.is_none()))
        .map(|room| {
            let (endpoint, problem) = resolve_room(&room);
            BrowserRow {
                room,
                endpoint,
                problem,
                is_local: false,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Resource)]
pub struct ManagerConfig {
    pub base_url: String,
}

impl ManagerConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: clean_base_url(&base_url.into()),
        }
    }

    // Runtime env wins (dev), then the saved config file, then the URL baked
    // in at release-build time by CI (`TRONZERO_MANAGER_URL`), then localhost
    // for bare dev builds.
    pub fn from_env_and_saved(saved: Option<String>) -> Self {
        Self::new(default_base_url(
            crate::platform::env_var("MANAGER_URL"),
            saved,
        ))
    }

    pub fn rooms_url(&self) -> String {
        format!("{}/api/rooms", self.base_url)
    }
}

// Precedence for the manager base URL, pure for testability: explicit value
// (the `MANAGER_URL` env var at runtime) beats the saved config value, which
// beats the build-time bake-in, which beats the localhost dev default.
fn default_base_url(env: Option<String>, saved: Option<String>) -> String {
    env.or(saved)
        .filter(|url| !url.trim().is_empty())
        .or_else(|| option_env!("TRONZERO_MANAGER_URL").map(str::to_string))
        .unwrap_or_else(|| "http://localhost:3001".into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchStatus {
    #[default]
    Loaded,
    Loading,
    Failed,
}

// Completed fetch handed from the ehttp callback to the frame poll.
type FetchSlot = Arc<Mutex<Option<Result<FetchOutcome, String>>>>;

#[derive(Debug, Clone)]
struct FetchOutcome {
    status: u16,
    ok: bool,
    body: Vec<u8>,
}

#[derive(Resource)]
pub struct BrowserState {
    /// Manager rooms only; combine with [`local_rows`] via [`merge_rows`].
    pub rows: Vec<BrowserRow>,
    pub status: FetchStatus,
    pub error: Option<String>,
    pub last_updated: Option<f64>,
    // Raw manager room count; lets the UI tell "manager empty" apart from
    // "only our own server registered".
    pub manager_total: usize,
    pending: Option<FetchSlot>,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            status: FetchStatus::Loaded,
            error: None,
            last_updated: None,
            manager_total: 0,
            pending: None,
        }
    }
}

impl BrowserState {
    // Fire a manager fetch unless one is already in flight.
    pub fn request_refresh(&mut self, config: &ManagerConfig) {
        if self.status == FetchStatus::Loading {
            return;
        }
        self.status = FetchStatus::Loading;
        let slot: FetchSlot = Arc::new(Mutex::new(None));
        self.pending = Some(Arc::clone(&slot));
        let url = config.rooms_url();
        let request = ehttp::Request::get(url.clone());
        ehttp::fetch(request, move |result| {
            let outcome = match result {
                Ok(response) => Ok(FetchOutcome {
                    status: response.status,
                    ok: response.ok,
                    body: response.bytes,
                }),
                Err(err) => Err(transport_error(&url, err)),
            };
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(outcome);
            }
        });
    }

    fn apply_rooms(&mut self, rooms: Vec<RoomInfo>, now: f64) {
        self.manager_total = rooms.len();
        self.rows = build_rows(rooms);
        self.status = FetchStatus::Loaded;
        self.error = None;
        self.last_updated = Some(now);
    }

    // Failures never touch the rows: the stale list stays selectable.
    fn apply_error(&mut self, message: String) {
        self.status = FetchStatus::Failed;
        self.error = Some(message);
    }
}

// Drain the in-flight fetch, if completed. Runs in PreUpdate before the menu
// so the Rooms screen always draws the freshest state.
pub fn poll_browser_fetch(mut browser: ResMut<BrowserState>, time: Res<Time<Real>>) {
    let Some(slot) = browser.pending.as_ref().map(Arc::clone) else {
        return;
    };
    let outcome = match slot.try_lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => return,
    };
    let Some(outcome) = outcome else {
        return; // still in flight
    };
    browser.pending = None;
    match outcome {
        Ok(response) if response.ok => match parse_rooms(&response.body) {
            Ok(rooms) => browser.apply_rooms(rooms, time.elapsed_secs_f64()),
            Err(message) => browser.apply_error(message),
        },
        Ok(response) => browser.apply_error(http_status_error(response.status)),
        // Transport failures already carry the attempted URL (built in
        // request_refresh, where the URL is known).
        Err(message) => browser.apply_error(message),
    }
}

// Eager first refresh so the Rooms screen is already populated on open.
pub fn initial_refresh(mut browser: ResMut<BrowserState>, config: Res<ManagerConfig>) {
    browser.request_refresh(&config);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, players: u32, heartbeat: u64) -> RoomInfo {
        RoomInfo {
            id: id.into(),
            host: "example.com".into(),
            port: 5000,
            web_port: None,
            cert_hash: None,
            display_name: id.into(),
            player_count: players,
            max_players: 8,
            bot_count: 0,
            last_heartbeat: heartbeat,
            ranked: false,
        }
    }

    #[test]
    fn room_info_ignores_secure_and_unknown_fields() {
        let body = br#"[{
            "id": "abc", "host": "example.com", "port": 5000,
            "secure": true, "displayName": "Arena",
            "playerCount": 3, "maxPlayers": 8,
            "extraFutureField": [1, 2]
        }]"#;
        let rooms = parse_rooms(body).unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].display_name, "Arena");
        assert_eq!(rooms[0].player_count, 3);
    }

    #[test]
    fn room_info_defaults_missing_heartbeat_to_zero() {
        let body = br#"[{
            "id": "abc", "host": "example.com", "port": 5000,
            "displayName": "Arena", "playerCount": 1, "maxPlayers": 8
        }]"#;
        assert_eq!(parse_rooms(body).unwrap()[0].last_heartbeat, 0);
    }

    #[test]
    fn bot_count_is_optional_and_never_affects_fullness() {
        let old = br#"[{
            "id": "a", "host": "h", "port": 1,
            "displayName": "Arena", "playerCount": 1, "maxPlayers": 4
        }]"#;
        assert_eq!(parse_rooms(old).unwrap()[0].bot_count, 0);
        let new = br#"[{
            "id": "a", "host": "h", "port": 1, "displayName": "Arena",
            "playerCount": 4, "botCount": 5, "maxPlayers": 4
        }]"#;
        let rooms = parse_rooms(new).unwrap();
        assert_eq!(rooms[0].bot_count, 5);
        assert!(rooms[0].is_full());
        let with_bots = RoomInfo {
            bot_count: 9,
            ..room("b", 3, 0)
        };
        assert!(!with_bots.is_full());
    }

    #[test]
    fn padded_host_is_trimmed_at_parse() {
        let body = br#"[{
            "id": "abc", "host": "  127.0.0.1 ", "port": 5000,
            "displayName": "Arena", "playerCount": 1, "maxPlayers": 8
        }]"#;
        assert_eq!(parse_rooms(body).unwrap()[0].host, "127.0.0.1");
    }

    fn local_server(port: u16) -> LocalServers {
        LocalServers::with_server(
            port,
            shared::probe::ProbeReply {
                protocol: shared::protocol::PROTOCOL_ID,
                players: 2,
                max_players: 10,
            },
        )
    }

    #[test]
    fn manager_rows_are_ordered_and_there_is_no_local_row_without_a_server() {
        let rows = build_rows(vec![
            room("quiet", 1, 9),
            room("busy-stale", 5, 1),
            room("busy-fresh", 5, 2),
        ]);
        let ids: Vec<&str> = rows.iter().map(|row| row.room.id.as_str()).collect();
        assert_eq!(ids, ["busy-fresh", "busy-stale", "quiet"]);
        assert!(local_rows(&LocalServers::default()).is_empty());
        assert!(BrowserState::default().rows.is_empty());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn detected_server_gets_a_connectable_local_row_first() {
        let local = local_rows(&local_server(5002));
        assert_eq!(local.len(), 1);
        assert!(local[0].is_local);
        assert_eq!(local[0].room.player_count, 2);
        assert_eq!(local[0].room.max_players, 10);
        assert_eq!(
            local[0].endpoint,
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 5002)).into())
        );
        let merged = merge_rows(local, &build_rows(vec![room("remote", 1, 1)]));
        assert!(merged[0].is_local);
        assert_eq!(merged[1].room.id, "remote");
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn server_from_another_build_is_listed_but_not_connectable() {
        let servers = LocalServers::with_server(
            5000,
            shared::probe::ProbeReply {
                protocol: shared::protocol::PROTOCOL_ID.wrapping_add(1),
                players: 0,
                max_players: 10,
            },
        );
        let rows = local_rows(&servers);
        assert!(rows[0].endpoint.is_none());
        assert!(rows[0].problem.is_some());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn manager_row_pointing_at_local_server_is_dropped() {
        let mut dup = room("dup", 9, 9);
        dup.host = "127.0.0.1".into();
        dup.port = 5002;
        let manager = build_rows(vec![dup.clone()]);
        let merged = merge_rows(local_rows(&local_server(5002)), &manager);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].is_local);
        // Without a detected local server the manager row is shown normally.
        let merged = merge_rows(Vec::new(), &manager);
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].is_local);
    }

    #[test]
    fn manager_total_counts_rooms_before_dedup() {
        let mut state = BrowserState::default();
        state.apply_rooms(vec![room("a", 1, 1)], 1.0);
        assert_eq!(state.manager_total, 1);
        state.apply_rooms(vec![], 2.0);
        assert_eq!(state.manager_total, 0);
    }

    #[test]
    fn fetch_error_keeps_stale_rows() {
        let mut state = BrowserState::default();
        state.apply_rooms(vec![room("a", 2, 3)], 1.0);
        assert_eq!(state.rows.len(), 1);
        state.apply_error(http_status_error(500));
        assert_eq!(state.status, FetchStatus::Failed);
        assert_eq!(state.error.as_deref(), Some("Manager returned 500"));
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].room.id, "a");
    }

    #[test]
    fn refresh_success_clears_error_and_stamps_time() {
        let mut state = BrowserState::default();
        state.apply_error("boom".into());
        state.apply_rooms(vec![], 42.0);
        assert_eq!(state.status, FetchStatus::Loaded);
        assert!(state.error.is_none());
        assert_eq!(state.last_updated, Some(42.0));
    }

    #[test]
    fn refresh_while_loading_is_ignored() {
        let mut state = BrowserState::default();
        let config = ManagerConfig::new("http://localhost:3001");
        state.request_refresh(&config);
        assert_eq!(state.status, FetchStatus::Loading);
        let first = state.pending.as_ref().map(Arc::as_ptr);
        state.request_refresh(&config);
        assert_eq!(state.pending.as_ref().map(Arc::as_ptr), first);
    }

    #[test]
    fn full_rooms_are_detected_but_unknown_capacity_is_not() {
        assert!(!room("a", 7, 0).is_full());
        assert!(room("a", 8, 0).is_full());
        assert!(!local_room(5000, None, None).is_full());
    }

    #[test]
    fn rooms_url_trims_trailing_slashes() {
        assert_eq!(
            ManagerConfig::new("http://localhost:3001///").rooms_url(),
            "http://localhost:3001/api/rooms"
        );
        assert_eq!(
            ManagerConfig::new("http://localhost:3001").rooms_url(),
            "http://localhost:3001/api/rooms"
        );
    }

    #[test]
    fn default_base_url_prefers_explicit_then_baked_in_then_localhost() {
        // Explicit value always wins, regardless of the build-time bake-in.
        assert_eq!(
            default_base_url(
                Some("http://explicit:3001".into()),
                Some("http://saved".into())
            ),
            "http://explicit:3001"
        );
        assert_eq!(
            default_base_url(None, Some("http://saved:3001".into())),
            "http://saved:3001"
        );
        // Without one, the release bake-in (if compiled in) or localhost.
        assert_eq!(
            default_base_url(None, None),
            option_env!("TRONZERO_MANAGER_URL").unwrap_or("http://localhost:3001")
        );
    }

    #[test]
    fn base_url_cleans_whitespace_and_cmd_quotes() {
        assert_eq!(
            ManagerConfig::new("  https://example.com:3001/ ").rooms_url(),
            "https://example.com:3001/api/rooms"
        );
        // cmd's `set VAR="..."` embeds the quotes in the value.
        assert_eq!(
            ManagerConfig::new("\"https://example.com:3001/\"").rooms_url(),
            "https://example.com:3001/api/rooms"
        );
        // Unbalanced quote is left alone, not silently mangled.
        assert_eq!(
            ManagerConfig::new("\"https://example.com:3001").rooms_url(),
            "\"https://example.com:3001/api/rooms"
        );
    }

    #[test]
    fn error_strings_match_js_client_wording() {
        assert_eq!(http_status_error(503), "Manager returned 503");
        let message = transport_error("http://localhost:3001/api/rooms", "connection refused");
        assert!(message.contains("http://localhost:3001/api/rooms"));
        assert!(message.contains("connection refused"));
    }

    #[test]
    fn room_info_reads_web_transport_fields() {
        let body = br#"[{
            "id": "abc", "host": "203.0.113.7", "port": 5000,
            "webPort": 5001, "certHash": "AB12",
            "displayName": "Arena", "playerCount": 1, "maxPlayers": 8
        }]"#;
        let room = &parse_rooms(body).unwrap()[0];
        assert_eq!(room.web_port, Some(5001));
        assert_eq!(room.cert_hash.as_deref(), Some("AB12"));
        let old = br#"[{
            "id": "abc", "host": "h", "port": 5000,
            "displayName": "Arena", "playerCount": 1, "maxPlayers": 8
        }]"#;
        assert_eq!(parse_rooms(old).unwrap()[0].web_port, None);
    }

    fn web_room(host: &str, web_port: Option<u16>, hash: Option<&str>) -> RoomInfo {
        RoomInfo {
            host: host.into(),
            web_port,
            cert_hash: hash.map(str::to_owned),
            ..room("w", 0, 0)
        }
    }

    #[test]
    fn browser_dials_web_port_with_lowercased_hash() {
        let (endpoint, problem) = web_endpoint(&web_room("203.0.113.7", Some(5001), Some("AB12")));
        assert!(problem.is_none());
        let endpoint = endpoint.unwrap();
        assert_eq!(endpoint.addr, "203.0.113.7:5001".parse().unwrap());
        assert_eq!(endpoint.cert_digest.as_deref(), Some("ab12"));
    }

    #[test]
    fn browser_rejects_rooms_it_cannot_dial() {
        for room in [
            web_room("203.0.113.7", None, Some("ab")),
            web_room("203.0.113.7", Some(5001), None),
            web_room("203.0.113.7", Some(5001), Some("")),
            web_room("play.example.com", Some(5001), Some("ab")),
        ] {
            let (endpoint, problem) = web_endpoint(&room);
            assert!(endpoint.is_none());
            assert!(problem.is_some());
        }
    }

    #[test]
    fn numeric_ip_resolves_without_dns() {
        let numeric = RoomInfo {
            host: "127.0.0.1".into(),
            port: 6000,
            ..room("n", 0, 0)
        };
        let (endpoint, error) = native_endpoint(&numeric);
        assert_eq!(
            endpoint,
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 6000)).into())
        );
        assert!(error.is_none());
    }
}
