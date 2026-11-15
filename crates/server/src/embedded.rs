//! A game server run on its own thread inside another process (the client's
//! "Host a server" button). The server is the same app the binary runs
//! (`new_app` + `add_game`), with the local-play rules; what only the binary
//! has (announce, ranked, web, signals, `process::exit`) is left out.
//!
//! [`spawn`] returns at once with a [`HostHandle`]; the thread reports through
//! [`HostStatus`]. Dropping the handle stops the server and joins the thread.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bevy::ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy::prelude::*;
use lightyear::prelude::Linked;
use lightyear::prelude::server::RawServer;

use crate::config::ServerConfig;
use crate::tutorial::StatusCell;

/// How long a port is given to answer a discovery probe before it counts as free.
const PROBE_WAIT: Duration = Duration::from_millis(50);
/// Updates given to the listener to open before the start counts as failed.
const STARTUP_UPDATES: u32 = 5;

/// Where the hosted server listens. An enum so another transport (in-memory,
/// for the browser) can be added without touching the callers.
#[derive(Clone, Debug)]
pub enum Transport {
    /// Raw UDP on `ip`, on the first of `ports` that is free.
    Udp { ip: IpAddr, ports: Vec<u16> },
}

impl Transport {
    /// All interfaces, so friends on the LAN can join.
    pub fn udp_any(ports: impl IntoIterator<Item = u16>) -> Self {
        Self::Udp {
            ip: Ipv4Addr::UNSPECIFIED.into(),
            ports: ports.into_iter().collect(),
        }
    }

    /// This machine only: nobody else can join.
    pub fn udp_loopback(ports: impl IntoIterator<Item = u16>) -> Self {
        Self::Udp {
            ip: Ipv4Addr::LOCALHOST.into(),
            ports: ports.into_iter().collect(),
        }
    }
}

/// What to host.
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub transport: Transport,
    pub mode: HostMode,
}

#[derive(Clone, Debug)]
pub enum HostMode {
    /// A match with the local-play rules, the built-in bots if `bots`.
    Play { bots: bool },
    /// The tutorial for one player; its status is [`HostHandle::tutorial`].
    Tutorial,
}

pub use crate::tutorial::{Fail, Phase, TutorialStatus};

/// Where a hosted server is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostStatus {
    Starting,
    Running { port: u16 },
    Failed(String),
    Stopping,
    Stopped,
}

/// A running (or starting, or finished) hosted server.
pub struct HostHandle {
    status: Arc<Mutex<HostStatus>>,
    stop: Arc<AtomicBool>,
    tutorial: StatusCell,
    thread: Option<JoinHandle<()>>,
}

impl HostHandle {
    pub fn status(&self) -> HostStatus {
        lock(&self.status).clone()
    }

    /// The tutorial's state as of its last tick; `None` before the first one
    /// and for a match.
    pub fn tutorial(&self) -> Option<TutorialStatus> {
        lock(&self.tutorial).clone()
    }

    /// Ask the server to stop; returns at once. The status turns `Stopped`
    /// when the thread is done and the port is free again.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut status = lock(&self.status);
        if matches!(*status, HostStatus::Starting | HostStatus::Running { .. }) {
            *status = HostStatus::Stopping;
        }
    }

    /// Whether the thread has ended (`Stopped` or `Failed`).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for HostHandle {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Start a server on a thread named `tron-host`.
pub fn spawn(config: HostConfig) -> HostHandle {
    let status = Arc::new(Mutex::new(HostStatus::Starting));
    let stop = Arc::new(AtomicBool::new(false));
    let tutorial = StatusCell::default();
    let spawned = {
        let (status, stop, tutorial) = (
            Arc::clone(&status),
            Arc::clone(&stop),
            Arc::clone(&tutorial),
        );
        std::thread::Builder::new()
            .name("tron-host".into())
            .spawn(move || run(config, status, stop, tutorial))
    };
    let thread = spawned
        .inspect_err(|e| *lock(&status) = HostStatus::Failed(format!("thread: {e}")))
        .ok();
    HostHandle {
        status,
        stop,
        tutorial,
        thread,
    }
}

/// The thread body: start, serve until asked to stop, report how it ended.
fn run(
    config: HostConfig,
    status: Arc<Mutex<HostStatus>>,
    stop: Arc<AtomicBool>,
    tutorial: StatusCell,
) {
    let _span = info_span!("host").entered();
    let result = catch_unwind(AssertUnwindSafe(|| {
        serve(&config, &status, &stop, &tutorial)
    }));
    let result = result.unwrap_or_else(|panic| Err(panic_message(panic.as_ref())));
    let end = final_status(result, stop.load(Ordering::Relaxed));
    if let HostStatus::Failed(reason) = &end {
        warn!("Hosted server failed: {reason}");
    }
    *lock(&status) = end;
}

/// How the thread ended. A stop that was asked for wins over any outcome.
fn final_status(result: Result<(), String>, stopped: bool) -> HostStatus {
    match result {
        _ if stopped => HostStatus::Stopped,
        Ok(()) => HostStatus::Stopped,
        Err(reason) => HostStatus::Failed(reason),
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

fn serve(
    config: &HostConfig,
    status: &Mutex<HostStatus>,
    stop: &Arc<AtomicBool>,
    tutorial: &StatusCell,
) -> Result<(), String> {
    let (mut app, port) = start(config, stop, tutorial)?;
    app.insert_resource(StopFlag(Arc::clone(stop)));
    app.add_systems(Last, exit_on_stop);
    {
        let mut status = lock(status);
        // A stop pressed while starting stays `Stopping`; the flag ends the run.
        if *status == HostStatus::Starting {
            *status = HostStatus::Running { port };
        }
    }
    info!(port, "Hosted server running");
    app.run();
    info!("Hosted server stopped");
    Ok(())
}

/// Build the app on the first usable port and check its listener opened.
fn start(
    config: &HostConfig,
    stop: &AtomicBool,
    tutorial: &StatusCell,
) -> Result<(App, u16), String> {
    let Transport::Udp { ip, ports } = &config.transport;
    let mut last = "no port to try".to_owned();
    for &port in ports {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped while starting".into());
        }
        if answers_probe(port) {
            last = format!("port {port} is taken by a game server");
            continue;
        }
        if let Err(e) = UdpSocket::bind((*ip, port)) {
            last = format!("port {port}: {e}");
            continue;
        }
        let mut app = build(config, SocketAddr::new(*ip, port), tutorial)?;
        if listening(&mut app) {
            return Ok((app, port));
        }
        last = format!("port {port}: the listener did not open");
    }
    Err(last)
}

fn build(config: &HostConfig, bind: SocketAddr, tutorial: &StatusCell) -> Result<App, String> {
    let mut app = crate::new_app();
    // A failed bind is an `Err` in an observer: log it, `listening` sees it.
    app.set_error_handler(bevy::ecs::error::warn);
    match config.mode {
        HostMode::Play { bots } => {
            let server = ServerConfig::local()?;
            let roster = if bots { server.roster()? } else { Vec::new() };
            let debug = crate::bot::BotDebugConfig::default();
            crate::add_game(&mut app, &server, bind, roster, debug)?;
        }
        HostMode::Tutorial => {
            let steps = crate::tutorial::builtin_steps()?;
            let director = crate::tutorial::Tutorial::new(steps, Arc::clone(tutorial));
            app.insert_resource(crate::admission::MaxPlayers(1));
            crate::add_core(&mut app, bind, crate::Director::Tutorial(director));
        }
    }
    // Share the client's task pools without competing for its cores: like the
    // binary, run every schedule on this thread.
    for (_, schedule) in app.world_mut().resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(SingleThreadedExecutor::new());
    }
    Ok(app)
}

/// Run the first updates (startup opens the listener); true once it is `Linked`.
fn listening(app: &mut App) -> bool {
    app.finish();
    app.cleanup();
    for _ in 0..STARTUP_UPDATES {
        app.update();
        let world = app.world_mut();
        if world
            .query_filtered::<(), (With<RawServer>, With<Linked>)>()
            .iter(world)
            .next()
            .is_some()
        {
            return true;
        }
    }
    false
}

/// Whether a game server already answers on this loopback port. Windows can
/// bind next to another process's wildcard bind, so a bind test is not enough.
fn answers_probe(port: u16) -> bool {
    let nonce = rand::random::<u32>().to_le_bytes();
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)) else {
        return false;
    };
    let sent = socket.send_to(
        &shared::probe::encode_request(nonce),
        (Ipv4Addr::LOCALHOST, port),
    );
    if sent.is_err() || socket.set_read_timeout(Some(PROBE_WAIT)).is_err() {
        return false;
    }
    let started = Instant::now();
    let mut buf = [0u8; 64];
    while started.elapsed() < PROBE_WAIT {
        // Any error (timeout, or ConnectionReset from a closed Windows port,
        // which fails at once) means nobody answers.
        let Ok(len) = socket.recv(&mut buf) else {
            return false;
        };
        if shared::probe::decode_reply(&buf[..len], nonce).is_some() {
            return true;
        }
    }
    false
}

#[derive(Resource)]
struct StopFlag(Arc<AtomicBool>);

fn exit_on_stop(flag: Res<StopFlag>, mut exit: MessageWriter<AppExit>) {
    if flag.0.load(Ordering::Relaxed) {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_wins_over_the_outcome() {
        assert_eq!(final_status(Ok(()), false), HostStatus::Stopped);
        assert_eq!(
            final_status(Err("x".into()), false),
            HostStatus::Failed("x".into())
        );
        assert_eq!(final_status(Err("x".into()), true), HostStatus::Stopped);
    }

    #[test]
    fn closed_port_does_not_answer_the_probe() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let started = Instant::now();
        assert!(!answers_probe(port));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
