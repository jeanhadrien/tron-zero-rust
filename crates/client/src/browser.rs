//! Server browser: room list from the Node manager plus a pinned localhost row.
//!
//! No UDP probing: "automatic local detection" is just a pinned
//! `127.0.0.1:5000` row that is always first and always connectable. Probing
//! would mean opening a socket per candidate and guessing from timeouts;
//! the manager's heartbeats already prove liveness, and stale entries sink
//! via the `last_heartbeat` sort. The local server's absence surfaces through
//! the existing `Session` timeout strings, not through new machinery.
//!
//! Fetch uses `ehttp` (egui's fetch lib): one API on native and wasm, no
//! async runtime, no threads or channels of our own. `request_refresh` fires
//! a request unless one is already in flight; `poll_browser_fetch` drains the
//! completed result once per frame. DNS resolution is a blocking
//! `ToSocketAddrs` call, but it runs once per refresh on receipt — never per
//! frame — so no worker thread is warranted. On wasm, resolution is skipped
//! and Connect is disabled: listing works over browser fetch, but play needs
//! the native UDP transport until WebTransport lands (see PLAN.md).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use serde::Deserialize;

const LOCAL_PORT: u16 = 5000;
const LOCAL_ID: &str = "local";

fn local_addr() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, LOCAL_PORT))
}

// One room as served by `GET /api/rooms` (see `Room.ts` in the JS repo).
// `secure` is deliberately absent: serde drops unknown fields, so omitting
// it makes accidental use of the geckos-only flag unrepresentable.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInfo {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub display_name: String,
    pub player_count: u32,
    pub max_players: u32,
    #[serde(default)]
    pub last_heartbeat: u64,
}

// One rendered row: the room plus its resolved game address, if any.
#[derive(Debug, Clone)]
pub struct BrowserRow {
    pub room: RoomInfo,
    pub addr: Option<SocketAddr>,
    pub resolve_error: Option<String>,
    pub is_local: bool,
}

// The pinned first row: needs no manager and no DNS.
pub fn local_row() -> BrowserRow {
    BrowserRow {
        room: RoomInfo {
            id: LOCAL_ID.into(),
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: LOCAL_PORT,
            display_name: "Localhost (built-in)".into(),
            player_count: 0,
            max_players: 0,
            last_heartbeat: 0,
        },
        addr: Some(local_addr()),
        resolve_error: None,
        is_local: true,
    }
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

// Parse the manager's room array; mirrors the JS client's `res.json()`.
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

// Mirrors the JS client's `Manager returned ${status}` wording.
pub fn http_status_error(status: u16) -> String {
    format!("Manager returned {status}")
}

pub fn transport_error(url: &str, err: impl std::fmt::Display) -> String {
    format!("Could not reach manager at {url}: {err}")
}

// Resolve a room's game address. Numeric IPs never touch DNS, so this is
// offline-safe for them; hostnames may block briefly, which is why this runs
// once per refresh on receipt, not per frame.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_room(room: &RoomInfo) -> (Option<SocketAddr>, Option<String>) {
    use std::net::ToSocketAddrs as _;
    match (room.host.as_str(), room.port).to_socket_addrs() {
        Ok(addrs) => {
            let mut addrs: Vec<SocketAddr> = addrs.collect();
            // Windows may resolve "localhost" to ::1 first; the server binds IPv4.
            addrs.sort_by_key(|addr| if addr.is_ipv4() { 0 } else { 1 });
            match addrs.into_iter().next() {
                Some(addr) => (Some(addr), None),
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

// wasm has no UDP transport yet: skip resolution, rows are listing-only.
#[cfg(target_arch = "wasm32")]
fn resolve_room(_room: &RoomInfo) -> (Option<SocketAddr>, Option<String>) {
    (None, None)
}

// UDP play needs a native build; wasm must wait for WebTransport.
pub const fn connect_supported() -> bool {
    cfg!(not(target_arch = "wasm32"))
}

// Local row first, then manager rooms; a manager entry pointing at the local
// server is dropped so the pinned row wins and never duplicates.
fn build_rows(rooms: Vec<RoomInfo>) -> Vec<BrowserRow> {
    let local = local_addr();
    let mut rows = vec![local_row()];
    for room in sort_manager_rows(rooms) {
        let (addr, resolve_error) = resolve_room(&room);
        if addr == Some(local) {
            continue;
        }
        rows.push(BrowserRow {
            room,
            addr,
            resolve_error,
            is_local: false,
        });
    }
    rows
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

    // Runtime env wins (dev), then the URL baked in at release-build time
    // by CI (`TRONZERO_MANAGER_URL`), then localhost for bare dev builds.
    pub fn from_env() -> Self {
        Self::new(default_base_url(std::env::var("MANAGER_URL").ok()))
    }

    pub fn rooms_url(&self) -> String {
        format!("{}/api/rooms", self.base_url)
    }
}

// Precedence for the manager base URL, pure for testability: explicit value
// (the `MANAGER_URL` env var at runtime) beats the build-time bake-in, which
// beats the localhost dev default.
fn default_base_url(env: Option<String>) -> String {
    env.or_else(|| option_env!("TRONZERO_MANAGER_URL").map(str::to_string))
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
    pub rows: Vec<BrowserRow>,
    pub status: FetchStatus,
    pub error: Option<String>,
    pub last_updated: Option<f64>,
    // Raw manager room count before local-duplicate filtering; lets the UI
    // tell "manager empty" apart from "only our own server registered".
    pub manager_total: usize,
    pending: Option<FetchSlot>,
}

impl Default for BrowserState {
    // Local row only: usable before the first refresh and when the manager
    // is unreachable.
    fn default() -> Self {
        Self {
            rows: vec![local_row()],
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
            display_name: id.into(),
            player_count: players,
            max_players: 8,
            last_heartbeat: heartbeat,
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
    fn padded_host_is_trimmed_at_parse() {
        let body = br#"[{
            "id": "abc", "host": "  127.0.0.1 ", "port": 5000,
            "displayName": "Arena", "playerCount": 1, "maxPlayers": 8
        }]"#;
        assert_eq!(parse_rooms(body).unwrap()[0].host, "127.0.0.1");
    }

    #[test]
    fn local_row_stays_first_and_manager_rows_are_ordered() {
        let rows = build_rows(vec![
            room("quiet", 1, 9),
            room("busy-stale", 5, 1),
            room("busy-fresh", 5, 2),
        ]);
        assert_eq!(rows.len(), 4);
        assert!(rows[0].is_local);
        assert!(rows[0].addr.is_some());
        let ids: Vec<&str> = rows[1..].iter().map(|row| row.room.id.as_str()).collect();
        assert_eq!(ids, ["busy-fresh", "busy-stale", "quiet"]);
    }

    #[test]
    fn manager_row_pointing_at_local_server_is_dropped() {
        let mut dup = room("dup", 9, 9);
        dup.host = "127.0.0.1".into();
        dup.port = LOCAL_PORT;
        let rows = build_rows(vec![dup]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_local);
    }

    #[test]
    fn own_server_only_still_counts_as_registered() {
        let mut dup = room("dup", 9, 9);
        dup.host = "127.0.0.1".into();
        dup.port = LOCAL_PORT;
        let mut state = BrowserState::default();
        state.apply_rooms(vec![dup], 1.0);
        assert!(state.rows.iter().all(|row| row.is_local));
        assert_eq!(state.manager_total, 1);
        state.apply_rooms(vec![], 2.0);
        assert_eq!(state.manager_total, 0);
    }

    #[test]
    fn fetch_error_keeps_stale_rows() {
        let mut state = BrowserState::default();
        state.apply_rooms(vec![room("a", 2, 3)], 1.0);
        assert_eq!(state.rows.len(), 2);
        state.apply_error(http_status_error(500));
        assert_eq!(state.status, FetchStatus::Failed);
        assert_eq!(state.error.as_deref(), Some("Manager returned 500"));
        assert_eq!(state.rows.len(), 2);
        assert_eq!(state.rows[1].room.id, "a");
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
            default_base_url(Some("http://explicit:3001".into())),
            "http://explicit:3001"
        );
        // Without one, the release bake-in (if compiled in) or localhost.
        assert_eq!(
            default_base_url(None),
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

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn numeric_ip_resolves_without_dns() {
        let numeric = RoomInfo {
            host: "127.0.0.1".into(),
            port: 6000,
            ..room("n", 0, 0)
        };
        let (addr, error) = resolve_room(&numeric);
        assert_eq!(addr, Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 6000))));
        assert!(error.is_none());
    }
}
