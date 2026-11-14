//! Match overlay: the round bar (counter and clock) and the banner of the
//! phases that interrupt play (countdown, round winner, match result). Every
//! server runs a match; a single-round one shows only its clock.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use shared::{MatchPhase, MatchStatus};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;
use crate::theme::Theme;

// The bar is drawn translucent like the rest of the HUD.
const HUD_ALPHA: f32 = 0.7;
/// Banner distance from the top: above the centered scoreboard of the end screen.
const BANNER_TOP: f32 = 96.0;

/// The top bar: "ROUND 2 / 5   1:12", only the clock for a single-round
/// match, "WAITING FOR PLAYERS" while no match runs.
pub fn header(status: &MatchStatus) -> String {
    if status.phase == MatchPhase::Free {
        return "WAITING FOR PLAYERS".to_owned();
    }
    let clock = status
        .secs_left
        .map(|secs| format!("{}:{:02}", secs / 60, secs % 60));
    let round =
        (status.rounds > 1).then(|| format!("ROUND {} / {}", status.round.max(1), status.rounds));
    [round, clock]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("   ")
}

/// Banner title and subtitle for the phases that interrupt play, `None` while
/// a round is being played or no match runs.
pub fn banner(status: &MatchStatus) -> Option<(String, String)> {
    let single = status.rounds <= 1;
    let secs = |text: &str| {
        status
            .secs_left
            .map_or(String::new(), |s| format!("{text} {s}"))
    };
    match status.phase {
        MatchPhase::Free | MatchPhase::Playing => None,
        MatchPhase::Starting if single => Some(("MATCH STARTS".to_owned(), secs("Starting in"))),
        MatchPhase::Starting => Some((
            format!("MATCH STARTS - {} ROUNDS", status.rounds),
            secs("Round 1 in"),
        )),
        MatchPhase::Intermission => Some((round_result(status), secs("Next round in"))),
        MatchPhase::MatchOver if single => Some((match_result(status), secs("New match in"))),
        MatchPhase::MatchOver => Some((
            round_result(status),
            status.secs_left.map_or("Final standings".to_owned(), |s| {
                format!("Final standings - new match in {s}")
            }),
        )),
    }
}

fn round_result(status: &MatchStatus) -> String {
    match &status.winner {
        Some(name) => format!("ROUND {} WON BY {}", status.round, name.to_uppercase()),
        None => format!("ROUND {} - NO WINNER", status.round),
    }
}

fn match_result(status: &MatchStatus) -> String {
    match &status.winner {
        Some(name) => format!("MATCH WON BY {}", name.to_uppercase()),
        None => "MATCH OVER - NO WINNER".to_owned(),
    }
}

/// Whether a dead rider may respawn with Space now: riding free while no
/// match runs, or during a round of a match with respawns.
pub fn respawn_allowed(status: &MatchStatus) -> bool {
    match status.phase {
        MatchPhase::Free => true,
        MatchPhase::Playing => status.respawns,
        MatchPhase::Starting | MatchPhase::Intermission | MatchPhase::MatchOver => false,
    }
}

pub fn round_ui(
    mut contexts: EguiContexts,
    session: Res<Session>,
    menu: Res<MenuState>,
    theme: Res<Theme>,
    status: Option<Single<&MatchStatus>>,
) {
    if session.phase() != ConnectionPhase::Playing || menu.open {
        return;
    }
    let Some(status) = status else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let head = header(&status);
    if !head.is_empty() {
        egui::Area::new("round_bar".into())
            .anchor(egui::Align2::CENTER_TOP, [0.0, 12.0])
            .interactable(false)
            .show(ctx, |ui| {
                let size = if status.phase == MatchPhase::Free {
                    13.0
                } else {
                    16.0
                };
                ui.label(
                    egui::RichText::new(head)
                        .size(size)
                        .strong()
                        .color(theme.body.egui().gamma_multiply(HUD_ALPHA)),
                );
            });
    }
    let Some((title, subtitle)) = banner(&status) else {
        return;
    };
    egui::Area::new("round_banner".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, BANNER_TOP])
        .interactable(false)
        .show(ctx, |ui| {
            theme.panel_frame().show(ui, |ui| {
                ui.set_min_width(384.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(30.0)
                            .strong()
                            .color(theme.accent.egui()),
                    );
                    if !subtitle.is_empty() {
                        ui.label(
                            egui::RichText::new(subtitle)
                                .size(18.0)
                                .color(theme.body.egui()),
                        );
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(rounds: u32, phase: MatchPhase, winner: Option<&str>) -> MatchStatus {
        MatchStatus {
            rounds,
            round: 2,
            phase,
            secs_left: Some(72),
            winner: winner.map(str::to_owned),
            respawns: false,
        }
    }

    #[test]
    fn the_bar_shows_the_round_counter_only_for_several_rounds() {
        assert_eq!(
            header(&status(5, MatchPhase::Playing, None)),
            "ROUND 2 / 5   1:12"
        );
        assert_eq!(header(&status(1, MatchPhase::Playing, None)), "1:12");
        let endless = MatchStatus {
            secs_left: None,
            ..status(1, MatchPhase::Playing, None)
        };
        assert_eq!(header(&endless), "");
        assert_eq!(
            header(&status(1, MatchPhase::Free, None)),
            "WAITING FOR PLAYERS"
        );
    }

    #[test]
    fn banners_name_the_winner_or_the_draw_and_only_interrupt_play() {
        assert_eq!(banner(&status(5, MatchPhase::Playing, None)), None);
        assert_eq!(banner(&status(5, MatchPhase::Free, None)), None);
        let (title, sub) = banner(&status(5, MatchPhase::Intermission, Some("Ann"))).unwrap();
        assert_eq!(title, "ROUND 2 WON BY ANN");
        assert_eq!(sub, "Next round in 72");
        let (title, _) = banner(&status(5, MatchPhase::Intermission, None)).unwrap();
        assert_eq!(title, "ROUND 2 - NO WINNER");
        let (title, sub) = banner(&status(5, MatchPhase::MatchOver, Some("Ann"))).unwrap();
        assert_eq!(title, "ROUND 2 WON BY ANN");
        assert_eq!(sub, "Final standings - new match in 72");
    }

    #[test]
    fn a_single_round_match_ends_with_the_match_result() {
        let (title, sub) = banner(&status(1, MatchPhase::MatchOver, Some("Ann"))).unwrap();
        assert_eq!(title, "MATCH WON BY ANN");
        assert_eq!(sub, "New match in 72");
        let (title, _) = banner(&status(1, MatchPhase::MatchOver, None)).unwrap();
        assert_eq!(title, "MATCH OVER - NO WINNER");
        let (title, _) = banner(&status(1, MatchPhase::Starting, None)).unwrap();
        assert_eq!(title, "MATCH STARTS");
    }

    #[test]
    fn respawns_follow_the_phase_and_the_match_rules() {
        assert!(respawn_allowed(&status(5, MatchPhase::Free, None)));
        assert!(!respawn_allowed(&status(5, MatchPhase::Playing, None)));
        let with_respawns = MatchStatus {
            respawns: true,
            ..status(1, MatchPhase::Playing, None)
        };
        assert!(respawn_allowed(&with_respawns));
        let over = MatchStatus {
            phase: MatchPhase::MatchOver,
            ..with_respawns
        };
        assert!(!respawn_allowed(&over));
    }
}
