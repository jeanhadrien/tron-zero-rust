//! The server the player hosts from the menu ("Host a server"). Desktop only:
//! the server runs on its own thread (`server::embedded`) and listens on all
//! interfaces, on the first free port of the local-scan range, so it also
//! shows up in the local server list and friends on the LAN can join. It
//! stops with the button or when the client exits. In the browser the module
//! compiles to an empty state and the menu hides the buttons.

// The server is native-only, so on wasm the bookkeeping is unused.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use bevy::prelude::*;
#[cfg(not(target_family = "wasm"))]
use server::embedded::HostStatus;

#[cfg(not(target_family = "wasm"))]
use crate::connection::ConnectionPhase;
use crate::connection::Session;

/// What the menu shows of the hosted server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HostView {
    #[default]
    Stopped,
    Starting,
    Running {
        port: u16,
    },
    Failed(String),
    Stopping,
}

/// What the "Host a server" button does when pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAction {
    Start,
    Stop,
}

/// The host buttons for one [`HostView`]: caption and action of the first one
/// (`None`: disabled), the port "Join my server" dials, and a status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPanel {
    pub caption: &'static str,
    pub action: Option<HostAction>,
    pub join_port: Option<u16>,
    pub line: Option<String>,
}

impl HostView {
    pub fn panel(&self) -> HostPanel {
        let (caption, action, join_port, line) = match self {
            Self::Stopped => ("Host a server", Some(HostAction::Start), None, None),
            Self::Starting => ("Starting server...", None, None, None),
            Self::Running { port } => (
                "Stop server",
                Some(HostAction::Stop),
                Some(*port),
                Some(format!("Hosting on port {port}")),
            ),
            Self::Failed(reason) => (
                "Host a server",
                Some(HostAction::Start),
                None,
                Some(format!("Server failed: {reason}")),
            ),
            Self::Stopping => ("Stopping server...", None, None, None),
        };
        HostPanel {
            caption,
            action,
            join_port,
            line,
        }
    }
}

/// The hosted server, if any, and what the menu knows about it.
#[derive(Resource, Default)]
pub struct HostState {
    view: HostView,
    /// The player dialed their own server from the menu.
    joined: bool,
    /// Stop pressed while joined: stop once the session is offline.
    stop_when_offline: bool,
    #[cfg(not(target_family = "wasm"))]
    handle: Option<server::embedded::HostHandle>,
}

impl HostState {
    pub fn view(&self) -> &HostView {
        &self.view
    }

    /// Note that the player joined their own server (see [`HostState::stop`]).
    pub fn joined_own(&mut self) {
        self.joined = true;
    }
}

#[cfg(not(target_family = "wasm"))]
impl HostState {
    /// Start hosting: bots on, all interfaces, first free local-scan port.
    pub fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }
        self.view = HostView::Starting;
        self.handle = Some(server::embedded::spawn(server::embedded::HostConfig {
            transport: server::embedded::Transport::udp_any(crate::local_scan::LOCAL_PORTS),
            bots: true,
        }));
    }

    /// Stop hosting. A player on their own server leaves it first, so the
    /// server gets the leave; one on another server stays connected.
    pub fn stop(&mut self, session: &mut Session) {
        if self.joined && session.phase() != ConnectionPhase::Offline {
            if session.phase() != ConnectionPhase::Disconnecting {
                session.disconnect("Disconnected from server.");
            }
            self.stop_when_offline = true;
        } else if let Some(handle) = &self.handle {
            handle.stop();
            self.view = HostView::Stopping;
        }
    }
}

#[cfg(target_family = "wasm")]
impl HostState {
    pub fn start(&mut self) {}

    pub fn stop(&mut self, _session: &mut Session) {}
}

/// Registers the state everywhere, and the systems that follow the thread on
/// the desktop.
pub fn add_host(app: &mut App) {
    app.init_resource::<HostState>();
    #[cfg(not(target_family = "wasm"))]
    {
        app.add_systems(PreUpdate, poll_host);
        app.add_systems(Last, stop_host_on_exit);
    }
}

/// Follow the server thread's status, and carry out a stop that waited for
/// the session to end.
#[cfg(not(target_family = "wasm"))]
fn poll_host(mut host: ResMut<HostState>, session: Res<Session>) {
    let host = host.bypass_change_detection();
    if session.phase() == ConnectionPhase::Offline {
        host.joined = false;
        if host.stop_when_offline {
            host.stop_when_offline = false;
            if let Some(handle) = &host.handle {
                handle.stop();
            }
        }
    }
    let Some(handle) = &host.handle else {
        return;
    };
    host.view = match handle.status() {
        HostStatus::Starting => HostView::Starting,
        HostStatus::Running { port } => HostView::Running { port },
        HostStatus::Failed(reason) => HostView::Failed(reason),
        HostStatus::Stopping => HostView::Stopping,
        HostStatus::Stopped => HostView::Stopped,
    };
    // A finished thread is joined at once; the view keeps the failure.
    if handle.is_finished() {
        host.handle = None;
    }
}

/// The client is quitting (its session is already over): stop the server and
/// wait for its thread, so the port is free and nothing outlives the process.
#[cfg(not(target_family = "wasm"))]
fn stop_host_on_exit(mut exits: MessageReader<AppExit>, mut host: ResMut<HostState>) {
    if exits.read().count() > 0 {
        // Dropping the handle stops the server and joins its thread.
        host.handle = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_follow_the_host_status() {
        let stopped = HostView::Stopped.panel();
        assert_eq!(stopped.action, Some(HostAction::Start));
        assert_eq!(stopped.join_port, None);

        for busy in [HostView::Starting, HostView::Stopping] {
            let panel = busy.panel();
            assert_eq!((panel.action, panel.join_port), (None, None));
        }

        let running = HostView::Running { port: 5002 }.panel();
        assert_eq!(running.caption, "Stop server");
        assert_eq!(running.action, Some(HostAction::Stop));
        assert_eq!(running.join_port, Some(5002));
        assert!(running.line.unwrap().contains("5002"));

        let failed = HostView::Failed("port 5000: in use".into()).panel();
        assert_eq!(failed.action, Some(HostAction::Start), "retry is allowed");
        assert!(failed.line.unwrap().contains("in use"));
    }
}
