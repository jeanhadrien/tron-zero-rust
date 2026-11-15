//! The tutorial, started from the menu. Desktop only: a server in tutorial
//! mode runs on its own thread (`server::embedded`), on loopback, and the
//! client joins it as soon as it listens. The server judges every attempt;
//! this module shows its status (prompt, clock, result) and the goal area, and
//! stops the server when the player leaves. In the browser the module compiles
//! to an inactive state and the menu hides the button.

// The server is native-only, so on wasm the bookkeeping is unused.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::ops::RangeInclusive;

use bevy::prelude::*;

/// Ports the tutorial server tries, outside the local-scan range so it never
/// shows up as a server to join.
pub const TUTORIAL_PORTS: RangeInclusive<u16> = 5010..=5013;
/// A step file to play instead of the shipped one (authoring and tests).
pub const TUTORIAL_FILE_ENV: &str = "TRON_TUTORIAL";

/// The tutorial server, if one runs, and what the client knows of it.
#[derive(Resource, Default)]
pub struct TutorialState {
    /// The client dialed the tutorial server.
    joined: bool,
    /// The server was asked to stop; it is forgotten once its thread ends.
    stopping: bool,
    /// Why the last start failed, for the menu.
    error: Option<String>,
    #[cfg(not(target_family = "wasm"))]
    handle: Option<server::embedded::HostHandle>,
    #[cfg(not(target_family = "wasm"))]
    status: Option<server::embedded::TutorialStatus>,
}

impl TutorialState {
    /// The player is in a tutorial: the match HUD (round bar, death panel) is
    /// hidden.
    pub fn active(&self) -> bool {
        #[cfg(not(target_family = "wasm"))]
        {
            self.joined && !self.stopping && self.handle.is_some()
        }
        #[cfg(target_family = "wasm")]
        {
            false
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

#[cfg(not(target_family = "wasm"))]
impl TutorialState {
    /// Start the tutorial server; the client joins it once it listens.
    pub fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }
        self.error = None;
        self.joined = false;
        self.stopping = false;
        self.status = None;
        let text = match steps_text() {
            Ok(text) => text,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.handle = Some(server::embedded::spawn(server::embedded::HostConfig {
            transport: server::embedded::Transport::udp_loopback(TUTORIAL_PORTS),
            mode: server::embedded::HostMode::Tutorial { text },
        }));
    }
}

/// The step file: `TRON_TUTORIAL` if set, else the shipped one. The server
/// checks it.
#[cfg(not(target_family = "wasm"))]
fn steps_text() -> Result<String, String> {
    match std::env::var_os(TUTORIAL_FILE_ENV).filter(|p| !p.is_empty()) {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", std::path::Path::new(&path).display())),
        None => Ok(server::embedded::TUTORIAL.to_owned()),
    }
}

#[cfg(target_family = "wasm")]
impl TutorialState {
    pub fn start(&mut self) {}
}

/// Registers the state everywhere, and on the desktop the systems that follow
/// the server and draw the tutorial.
pub fn add_tutorial(app: &mut App) {
    app.init_resource::<TutorialState>();
    #[cfg(not(target_family = "wasm"))]
    {
        app.add_systems(PreUpdate, poll_tutorial);
        app.add_systems(Update, draw_goal.in_set(crate::render::RenderSet::Draw));
        // Under the menu, like the rest of the HUD.
        app.add_systems(
            bevy_egui::EguiPrimaryContextPass,
            native::panel
                .after(crate::theme::apply_egui_theme)
                .before(crate::menu::menu_ui),
        );
        app.add_systems(Last, stop_tutorial_on_exit);
    }
}

/// Join the server once it listens, follow its status, and stop it when the
/// player is back offline.
#[cfg(not(target_family = "wasm"))]
fn poll_tutorial(
    mut state: ResMut<TutorialState>,
    mut session: ResMut<crate::connection::Session>,
    name: Res<crate::settings::PlayerName>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    use crate::connection::ConnectionPhase;
    use server::embedded::HostStatus;

    let state = state.bypass_change_detection();
    let Some(handle) = &state.handle else {
        return;
    };
    let offline = session.phase() == ConnectionPhase::Offline;
    match handle.status() {
        HostStatus::Running { port } if !state.joined && offline => {
            let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
            let name = shared::protocol::sanitize_name(&name.0);
            session.connect(addr, &name, time.elapsed_secs_f64(), &mut commands);
            state.joined = true;
        }
        HostStatus::Failed(reason) => {
            state.error = Some(reason);
            state.handle = None;
            state.joined = false;
            if offline {
                session.status = crate::connection::IDLE_STATUS.into();
            }
            return;
        }
        // Left the tutorial, or went to another server before joining it:
        // stop its server, and forget it once the thread is done, whatever
        // the session does meanwhile.
        _ if state.stopping || (state.joined && offline) || (!state.joined && !offline) => {
            handle.stop();
            state.stopping = true;
            if handle.is_finished() {
                state.handle = None;
                state.status = None;
                state.joined = false;
                state.stopping = false;
                return;
            }
        }
        _ => {}
    }
    state.status = handle.tutorial();
}

/// The client is quitting: stop the server and wait for its thread.
#[cfg(not(target_family = "wasm"))]
fn stop_tutorial_on_exit(mut exits: MessageReader<AppExit>, mut state: ResMut<TutorialState>) {
    if exits.read().count() > 0 {
        state.handle = None;
    }
}

/// Outline the goal area while an attempt runs.
#[cfg(not(target_family = "wasm"))]
fn draw_goal(state: Res<TutorialState>, theme: Res<crate::theme::Theme>, mut gizmos: Gizmos) {
    use server::embedded::Phase;
    let Some(status) = &state.status else {
        return;
    };
    let Some(area) = status.goal_area else {
        return;
    };
    if matches!(status.phase, Phase::Countdown | Phase::Playing) {
        gizmos.rect_2d(area.center(), area.size(), theme.accent.bevy());
    }
}

#[cfg(not(target_family = "wasm"))]
mod native {
    use bevy::prelude::Res;
    use bevy_egui::{EguiContexts, egui};
    use server::embedded::{Phase, TutorialStatus};

    use super::TutorialState;
    use crate::menu::MenuState;
    use crate::theme::Theme;

    /// Title and detail line of the panel for a status.
    pub fn lines(status: &TutorialStatus) -> (String, String) {
        let secs = status.ticks_left.div_ceil(shared::TICK_HZ as u32);
        let step = format!("STEP {} / {}", status.step + 1, status.steps);
        match status.phase {
            Phase::Waiting => ("TUTORIAL".into(), "Joining...".into()),
            Phase::Countdown => (step, format!("{}   Go in {secs}", status.prompt)),
            Phase::Playing => (step, format!("{}   {secs}s", status.prompt)),
            Phase::Passed => (step, "Well done!".into()),
            Phase::Failed => (
                step,
                format!(
                    "{} Try again.",
                    status
                        .fail
                        .as_ref()
                        .map_or("Failed.".into(), |fail| fail.label())
                ),
            ),
            Phase::Done => (
                "TUTORIAL COMPLETE".into(),
                "Open the menu (Escape) to leave.".into(),
            ),
        }
    }

    /// The tutorial panel at the top of the screen, hidden under the menu.
    pub fn panel(
        mut contexts: EguiContexts,
        state: Res<TutorialState>,
        menu: Res<MenuState>,
        theme: Res<Theme>,
    ) {
        let Some(status) = &state.status else {
            return;
        };
        if menu.open {
            return;
        }
        let Ok(ctx) = contexts.ctx_mut() else {
            return;
        };
        let (title, detail) = lines(status);
        egui::Area::new("tutorial_panel".into())
            .anchor(egui::Align2::CENTER_TOP, [0.0, 12.0])
            .interactable(false)
            .show(ctx, |ui| {
                theme.panel_frame().show(ui, |ui| {
                    ui.set_min_width(384.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(title)
                                .size(22.0)
                                .strong()
                                .color(theme.accent.egui()),
                        );
                        ui.label(
                            egui::RichText::new(detail)
                                .size(16.0)
                                .color(theme.body.egui()),
                        );
                    });
                });
            });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use server::embedded::Fail;

        #[test]
        fn the_panel_says_what_the_server_said() {
            let status = TutorialStatus {
                step: 0,
                steps: 2,
                phase: Phase::Playing,
                ticks_left: 121,
                prompt: "Reach it.".into(),
                ..TutorialStatus::default()
            };
            assert_eq!(
                lines(&status),
                ("STEP 1 / 2".into(), "Reach it.   2s".into())
            );
            let failed = TutorialStatus {
                phase: Phase::Failed,
                fail: Some(Fail::Timeout),
                ..status
            };
            assert!(lines(&failed).1.starts_with("Out of time."));
        }
    }
}
