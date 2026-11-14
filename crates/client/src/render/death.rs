//! Centered death prompt drawn with egui, reusing the menu panel style.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, ControlledBy, Predicted};
use shared::protocol::RespawnOutcome;
use shared::{IsAlive, MatchPhase, MatchStatus, Player, PlayerInput};

use crate::connection::{ConnectionPhase, Session};
use crate::input::RespawnUi;
use crate::menu::MenuState;
use crate::theme::Theme;

// Pure text logic; unchanged from the Bevy-UI version.
pub fn prompt(alive: bool, respawn: &RespawnUi) -> &'static str {
    if alive {
        ""
    } else if respawn.pending_generation.is_some() {
        "Respawn requested — waiting for server..."
    } else {
        match respawn.outcome {
            Some(RespawnOutcome::WaitForNextRound) => WAIT_TEXT,
            Some(RespawnOutcome::NoSafeSpace) => "No safe spawn space. Press Space to retry.",
            Some(RespawnOutcome::NotEligible) => {
                "Server has not confirmed eligibility. Press Space to retry."
            }
            _ => "Press Space to respawn",
        }
    }
}

const WAIT_TEXT: &str = "Waiting for the next round...";

// Visibility + text in one pure helper so tests skip the egui context:
// hidden while the menu is open or outside Playing, else the prompt above.
// While a rounds match runs nobody respawns: the dead wait, and the round
// banner replaces this panel between rounds.
pub fn death_message(
    phase: ConnectionPhase,
    menu_open: bool,
    alive: Option<bool>,
    respawn: &RespawnUi,
    round: Option<&MatchStatus>,
) -> &'static str {
    if menu_open || phase != ConnectionPhase::Playing {
        return "";
    }
    match (alive, round) {
        (None, _) => "",
        (Some(alive), Some(status)) if status.phase != MatchPhase::Free => {
            if alive || status.phase != MatchPhase::Playing {
                ""
            } else {
                WAIT_TEXT
            }
        }
        (Some(alive), _) => prompt(alive, respawn),
    }
}

// Headline of the panel: a crash in free-for-all, merely being out in rounds
// (a late joiner has not crashed).
fn title(round: Option<&MatchStatus>) -> &'static str {
    if super::round::waits_for_round(round) {
        "OUT OF THE ROUND"
    } else {
        "YOU DIED"
    }
}

pub fn death_ui(
    mut contexts: EguiContexts,
    session: Res<Session>,
    menu: Res<MenuState>,
    theme: Res<Theme>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    players: Query<
        (&IsAlive, Option<&ControlledBy>),
        (
            With<Player>,
            With<Predicted>,
            With<InputMarker<PlayerInput>>,
        ),
    >,
    respawn: Res<RespawnUi>,
    round: Option<Single<&MatchStatus>>,
) {
    let round = round.as_deref().copied();
    let local = clients.iter().next().and_then(|client| {
        players
            .iter()
            .find(|(_, controlled)| controlled.is_none_or(|c| c.owner == client))
    });
    let text = death_message(
        session.phase(),
        menu.open,
        local.map(|(alive, _)| alive.0),
        &respawn,
        round,
    );
    if text.is_empty() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // Same panel look as the menu; default Middle order plus the menu's
    // own visibility rule (menu.open hides this) keeps the menu on top.
    egui::Area::new("death_prompt".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .interactable(false)
        .show(ctx, |ui| {
            theme.panel_frame().show(ui, |ui| {
                ui.set_min_width(384.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(title(round))
                            .size(36.0)
                            .strong()
                            .color(theme.danger.egui()),
                    );
                    ui.label(
                        egui::RichText::new(text)
                            .size(18.0)
                            .color(theme.body.egui()),
                    );
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::{Controlled, PeerId, RemoteId};

    #[test]
    fn death_panel_tracks_life_menu_and_disconnect() {
        let mut world = World::new();
        world.init_resource::<Session>();
        world.init_resource::<MenuState>();
        world.init_resource::<RespawnUi>();
        world.init_resource::<Probe>();
        world.spawn((Client::default(), RemoteId(PeerId::Server), Connected));
        let rider = world
            .spawn((
                Player,
                Controlled,
                Predicted,
                InputMarker::<PlayerInput>::default(),
                IsAlive(false),
            ))
            .id();
        world
            .resource_mut::<Session>()
            .force_phase(ConnectionPhase::Playing);
        world.resource_mut::<MenuState>().open = false;
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0.is_empty());

        world.resource_mut::<RespawnUi>().pending_generation = Some(1);
        world.run_system_once(update_probe).unwrap();
        assert!(world.resource::<Probe>().0.contains("waiting for server"));

        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(update_probe).unwrap();
        assert!(world.resource::<Probe>().0.is_empty());

        world.resource_mut::<MenuState>().open = false;
        world.entity_mut(rider).insert(IsAlive(true));
        world.run_system_once(update_probe).unwrap();
        assert!(world.resource::<Probe>().0.is_empty());

        world.entity_mut(rider).insert(IsAlive(false));
        world
            .resource_mut::<Session>()
            .force_phase(ConnectionPhase::Disconnecting);
        world.run_system_once(update_probe).unwrap();
        assert!(world.resource::<Probe>().0.is_empty());
    }

    #[test]
    fn prompt_tracks_dead_pending_rejected_and_alive_states() {
        let mut ui = RespawnUi::default();
        assert!(prompt(false, &ui).contains("Press Space"));
        ui.pending_generation = Some(1);
        assert!(prompt(false, &ui).contains("waiting for server"));
        assert_eq!(prompt(true, &ui), "");
        ui.pending_generation = None;
        ui.outcome = Some(RespawnOutcome::NoSafeSpace);
        assert!(prompt(false, &ui).contains("No safe spawn"));
        ui.outcome = Some(RespawnOutcome::NotEligible);
        assert!(prompt(false, &ui).contains("eligibility"));
    }

    #[test]
    fn in_rounds_mode_the_dead_wait_and_the_banner_covers_the_breaks() {
        let respawn = RespawnUi::default();
        let status = |phase| MatchStatus {
            rounds: 3,
            round: 1,
            phase,
            ..MatchStatus::default()
        };
        let text = |phase| {
            death_message(
                ConnectionPhase::Playing,
                false,
                Some(false),
                &respawn,
                Some(&status(phase)),
            )
        };
        assert!(text(MatchPhase::Playing).contains("next round"));
        assert!(text(MatchPhase::Intermission).is_empty());
        assert!(text(MatchPhase::MatchOver).is_empty());
        // Waiting for opponents is free riding: Space respawns as before.
        assert!(text(MatchPhase::Free).contains("Press Space"));
        let mut refused = RespawnUi::default();
        refused.outcome = Some(RespawnOutcome::WaitForNextRound);
        assert!(prompt(false, &refused).contains("next round"));
    }

    // Mirrors death_ui's lookup + visibility without an egui context.
    #[derive(Resource, Default)]
    struct Probe(String);

    fn update_probe(
        session: Res<Session>,
        menu: Res<MenuState>,
        clients: Query<Entity, (With<Client>, With<Connected>)>,
        players: Query<
            (&IsAlive, Option<&ControlledBy>),
            (
                With<Player>,
                With<Predicted>,
                With<InputMarker<PlayerInput>>,
            ),
        >,
        respawn: Res<RespawnUi>,
        round: Option<Single<&MatchStatus>>,
        probe: Option<ResMut<Probe>>,
    ) {
        let local = clients.iter().next().and_then(|client| {
            players
                .iter()
                .find(|(_, controlled)| controlled.is_none_or(|c| c.owner == client))
        });
        let text = death_message(
            session.phase(),
            menu.open,
            local.map(|(alive, _)| alive.0),
            &respawn,
            round.as_deref().copied(),
        );
        if let Some(mut probe) = probe {
            probe.0 = text.to_owned();
        }
    }
}
