//! Ranked play from the client: the manager account, join tickets and the
//! rating / leaderboard readouts.
//!
//! The account is anonymous and created on the first ranked join (`POST
//! /api/accounts`); its credential is kept in the client config, one per
//! manager URL. A ranked server only admits players holding a short-lived
//! ticket the manager signs for that room, so every ranked connect first asks
//! for a fresh one (`POST /api/tickets`) and hands it to the session, which
//! sends it right before the hello.
//!
//! HTTP goes through `ehttp` like the server browser: a request fills a shared
//! slot from its callback and a per-frame system drains it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::browser::{ManagerConfig, RoomInfo, resolve_room};
use crate::connection::{Endpoint, Session};
use crate::menu::MenuState;

/// Longest a ranked join waits on the manager before giving up.
const REQUEST_TIMEOUT_SECS: f64 = 15.0;
const LEADERBOARD_ROWS: u32 = 20;

/// The credential of an anonymous manager account (`<id>.<secret>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAccount {
    pub id: String,
    pub credential: String,
}

/// Saved accounts, keyed by manager base URL (an account only exists on the
/// manager that created it). Persisted by `config::persist_preferences`.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct Accounts(pub BTreeMap<String, SavedAccount>);

type Slot<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Fire `request`; `parse` turns a successful body into the result. Both run
/// on the network callback, the caller only polls the returned slot.
fn send<T: Send + 'static>(
    request: ehttp::Request,
    parse: impl FnOnce(&[u8]) -> Result<T, String> + Send + 'static,
) -> Slot<T> {
    let slot: Slot<T> = Arc::default();
    let out = Arc::clone(&slot);
    ehttp::fetch(request, move |result| {
        let outcome = match result {
            Ok(response) if response.ok => parse(&response.bytes),
            Ok(response) => Err(error_text(response.status, &response.bytes)),
            Err(err) => Err(format!("Could not reach the lobby manager: {err}")),
        };
        if let Ok(mut guard) = out.lock() {
            *guard = Some(outcome);
        }
    });
    slot
}

/// The finished result, if any (once: it is taken out of the slot).
fn take<T>(slot: &Slot<T>) -> Option<Result<T, String>> {
    slot.try_lock().ok()?.take()
}

/// The manager's `{"error": "..."}` message, else a generic one.
fn error_text(status: u16, body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: String,
    }
    serde_json::from_slice::<Body>(body)
        .map(|body| body.error)
        .unwrap_or_else(|_| format!("The lobby manager answered HTTP {status}."))
}

fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|err| format!("Unexpected reply from the manager: {err}"))
}

fn json_post(url: String, body: serde_json::Value, credential: Option<&str>) -> ehttp::Request {
    let request = ehttp::Request::new(
        ehttp::Method::POST,
        url,
        &[("Accept", "*/*"), ("Content-Type", "application/json")],
    )
    .with_body(body.to_string().into_bytes());
    with_credential(request, credential)
}

fn with_credential(request: ehttp::Request, credential: Option<&str>) -> ehttp::Request {
    match credential {
        Some(credential) => request.with_header("Authorization", format!("Bearer {credential}")),
        None => request,
    }
}

fn create_account(config: &ManagerConfig, name: &str) -> Slot<SavedAccount> {
    let url = format!("{}/api/accounts", config.base_url);
    send(
        json_post(url, serde_json::json!({ "displayName": name }), None),
        parse_json,
    )
}

/// A join ticket for `room_id`, decoded from the manager's base64.
fn ask_ticket(
    config: &ManagerConfig,
    account: &SavedAccount,
    room_id: &str,
    name: &str,
) -> Slot<Vec<u8>> {
    #[derive(Deserialize)]
    struct Reply {
        ticket: String,
    }
    let url = format!("{}/api/tickets", config.base_url);
    let body = serde_json::json!({ "roomId": room_id, "displayName": name });
    send(json_post(url, body, Some(&account.credential)), |bytes| {
        let reply: Reply = parse_json(bytes)?;
        base64::engine::general_purpose::STANDARD
            .decode(reply.ticket)
            .map_err(|_| "The manager sent a malformed ticket.".to_owned())
    })
}

/// The manager's pick for "Play now": a room and a ticket for it.
struct Placement {
    room: RoomInfo,
    ticket: Vec<u8>,
}

/// Ask the manager to choose a ranked room (`POST /api/play`); `exclude` lists
/// rooms that already turned this player away.
fn ask_placement(
    config: &ManagerConfig,
    account: &SavedAccount,
    name: &str,
    exclude: &[String],
) -> Slot<Placement> {
    #[derive(Deserialize)]
    struct Reply {
        room: RoomInfo,
        ticket: String,
    }
    let url = format!("{}/api/play", config.base_url);
    let body = serde_json::json!({ "exclude": exclude, "displayName": name });
    send(json_post(url, body, Some(&account.credential)), |bytes| {
        let reply: Reply = parse_json(bytes)?;
        let ticket = base64::engine::general_purpose::STANDARD
            .decode(reply.ticket)
            .map_err(|_| "The manager sent a malformed ticket.".to_owned())?;
        Ok(Placement {
            room: reply.room,
            ticket,
        })
    })
}

enum Step {
    Account(Slot<SavedAccount>),
    Ticket(Slot<Vec<u8>>),
    Place(Slot<Placement>),
}

/// Where a ranked join is headed.
enum Target {
    /// A room the player picked in the browser.
    Room { endpoint: Endpoint, room_id: String },
    /// "Play now": the manager picks the room.
    Play { exclude: Vec<String> },
}

struct PendingJoin {
    target: Target,
    name: String,
    step: Step,
    started_at: f64,
}

/// A ranked connect in progress: sign-up if needed, then a ticket (or a
/// placement, which includes one), then the actual connection.
#[derive(Resource, Default)]
pub struct RankedJoin {
    pending: Option<PendingJoin>,
    /// The room "Play now" last chose, so a retry after a refusal can skip it.
    pub last_placed: Option<String>,
}

impl RankedJoin {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Begin joining `room_id` at `endpoint` as `name`. Ignored while another
    /// ranked join is under way.
    pub fn start(
        &mut self,
        endpoint: Endpoint,
        room_id: String,
        name: String,
        config: &ManagerConfig,
        accounts: &Accounts,
        now: f64,
    ) {
        let target = Target::Room { endpoint, room_id };
        self.begin(target, name, config, accounts, now);
    }

    /// "Play now": let the manager choose a ranked room, skipping `exclude`.
    pub fn start_play(
        &mut self,
        exclude: Vec<String>,
        name: String,
        config: &ManagerConfig,
        accounts: &Accounts,
        now: f64,
    ) {
        self.begin(Target::Play { exclude }, name, config, accounts, now);
    }

    fn begin(
        &mut self,
        target: Target,
        name: String,
        config: &ManagerConfig,
        accounts: &Accounts,
        now: f64,
    ) {
        if self.busy() {
            return;
        }
        let step = match accounts.0.get(&config.base_url) {
            Some(account) => first_step(&target, config, account, &name),
            None => Step::Account(create_account(config, &name)),
        };
        self.pending = Some(PendingJoin {
            target,
            name,
            step,
            started_at: now,
        });
    }
}

/// The request that follows having an account.
fn first_step(target: &Target, config: &ManagerConfig, account: &SavedAccount, name: &str) -> Step {
    match target {
        Target::Room { room_id, .. } => Step::Ticket(ask_ticket(config, account, room_id, name)),
        Target::Play { exclude } => Step::Place(ask_placement(config, account, name, exclude)),
    }
}

/// What a polled step decided.
enum Progress {
    Waiting,
    Next(Step),
    Connect {
        endpoint: Endpoint,
        ticket: Vec<u8>,
        placed_in: Option<String>,
    },
    Failed(String),
}

/// Advance the ranked join: store a new account, trade it for a ticket (or a
/// placement), then connect with the ticket. Failures land in the session status.
pub fn poll_ranked_join(
    mut join: ResMut<RankedJoin>,
    mut accounts: ResMut<Accounts>,
    mut session: ResMut<Session>,
    mut menu: ResMut<MenuState>,
    config: Res<ManagerConfig>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let Some(pending) = join.pending.as_mut() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let progress = match &pending.step {
        _ if now - pending.started_at > REQUEST_TIMEOUT_SECS => {
            Progress::Failed("The lobby manager did not answer in time.".into())
        }
        Step::Account(slot) => match take(slot) {
            None => Progress::Waiting,
            Some(Ok(account)) => {
                let next = first_step(&pending.target, &config, &account, &pending.name);
                accounts.0.insert(config.base_url.clone(), account);
                Progress::Next(next)
            }
            Some(Err(message)) => Progress::Failed(message),
        },
        Step::Ticket(slot) => match (take(slot), &pending.target) {
            (None, _) => Progress::Waiting,
            (Some(Ok(ticket)), Target::Room { endpoint, .. }) => Progress::Connect {
                endpoint: endpoint.clone(),
                ticket,
                placed_in: None,
            },
            (Some(Ok(_)), Target::Play { .. }) => Progress::Failed("Unexpected ticket.".into()),
            (Some(Err(message)), _) => Progress::Failed(message),
        },
        Step::Place(slot) => match take(slot) {
            None => Progress::Waiting,
            Some(Ok(Placement { room, ticket })) => match resolve_room(&room) {
                (Some(endpoint), _) => Progress::Connect {
                    endpoint,
                    ticket,
                    placed_in: Some(room.id),
                },
                (None, problem) => Progress::Failed(
                    problem.unwrap_or_else(|| "The chosen server cannot be reached.".into()),
                ),
            },
            Some(Err(message)) => Progress::Failed(message),
        },
    };
    match progress {
        Progress::Waiting => {}
        Progress::Next(step) => pending.step = step,
        Progress::Connect {
            endpoint,
            ticket,
            placed_in,
        } => {
            if let Some(done) = join.pending.take() {
                session.connect_with_ticket(endpoint, &done.name, ticket, now, &mut commands);
            }
            join.last_placed = placed_in;
        }
        Progress::Failed(message) => {
            join.pending = None;
            warn!(%message, "Ranked sign-in failed");
            session.status = format!("Ranked sign-in failed: {message}");
            menu.open = true;
        }
    }
}

/// The player's own standing, as served by `GET /api/me`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub rating: f64,
    pub rated_events: u32,
    /// Absent until the account is ranked (placement not finished).
    #[serde(default)]
    pub rank: Option<u32>,
}

impl Me {
    pub fn summary(&self) -> String {
        match self.rank {
            Some(rank) => format!("Rating {:.0} - rank #{rank}", self.rating),
            None => format!("Rating {:.0} - unranked", self.rating),
        }
    }
}

/// One line of `GET /api/leaderboard`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardRow {
    pub rank: u32,
    pub display_name: String,
    pub rating: f64,
    /// `human` or `bot`.
    #[serde(default)]
    pub kind: String,
}

/// Rating readouts, refreshed when the leaderboard opens and at startup.
#[derive(Resource, Default)]
pub struct Standing {
    pub me: Option<Me>,
    pub board: Vec<BoardRow>,
    pub error: Option<String>,
    me_slot: Option<Slot<Me>>,
    board_slot: Option<Slot<Vec<BoardRow>>>,
}

impl Standing {
    pub fn loading(&self) -> bool {
        self.me_slot.is_some() || self.board_slot.is_some()
    }

    /// Re-fetch the leaderboard and, with an account, the player's own rating.
    pub fn refresh(&mut self, config: &ManagerConfig, accounts: &Accounts) {
        if self.loading() {
            return;
        }
        let base = &config.base_url;
        let url = format!("{base}/api/leaderboard?limit={LEADERBOARD_ROWS}");
        self.board_slot = Some(send(ehttp::Request::get(url), parse_json));
        if let Some(account) = accounts.0.get(base) {
            let request = ehttp::Request::get(format!("{base}/api/me"));
            let request = with_credential(request, Some(&account.credential));
            self.me_slot = Some(send(request, parse_json));
        }
    }
}

/// Drain finished standing requests.
pub fn poll_standing(mut standing: ResMut<Standing>) {
    if let Some(result) = standing.board_slot.as_ref().and_then(take) {
        standing.board_slot = None;
        match result {
            Ok(board) => {
                standing.board = board;
                standing.error = None;
            }
            Err(message) => standing.error = Some(message),
        }
    }
    if let Some(result) = standing.me_slot.as_ref().and_then(take) {
        standing.me_slot = None;
        // A failure keeps the previous rating on screen; the board reports errors.
        if let Ok(me) = result {
            standing.me = Some(me);
        }
    }
}

/// Startup: show the saved account's rating without opening the leaderboard.
pub fn initial_standing(
    mut standing: ResMut<Standing>,
    config: Res<ManagerConfig>,
    accounts: Res<Accounts>,
) {
    standing.refresh(&config, &accounts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_errors_and_replies_parse() {
        assert_eq!(
            error_text(429, br#"{"error":"Too many requests"}"#),
            "Too many requests"
        );
        assert!(error_text(502, b"<html>").contains("502"));
        let me: Me = parse_json(
            br#"{"id":"u1","displayName":"Neo","rating":1523.4,"ratedEvents":12,"rank":3}"#,
        )
        .unwrap();
        assert_eq!(me.summary(), "Rating 1523 - rank #3");
        let unranked: Me = parse_json(br#"{"rating":1500,"ratedEvents":0}"#).unwrap();
        assert_eq!(unranked.summary(), "Rating 1500 - unranked");
        let board: Vec<BoardRow> = parse_json(
            br#"[{"rank":1,"id":"a","kind":"human","displayName":"Neo","rating":1600}]"#,
        )
        .unwrap();
        assert_eq!(board[0].display_name, "Neo");
        assert!(parse_json::<Me>(b"nope").is_err());
    }

    #[test]
    fn tickets_decode_from_base64() {
        let bytes = [0u8, 1, 2, 250, 251];
        let body = serde_json::json!({
            "ticket": base64::engine::general_purpose::STANDARD.encode(bytes)
        });
        #[derive(Deserialize)]
        struct Reply {
            ticket: String,
        }
        let reply: Reply = parse_json(body.to_string().as_bytes()).unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(reply.ticket)
            .unwrap();
        assert_eq!(decoded, bytes);
    }
}
