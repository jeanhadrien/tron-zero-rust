//! Rounds mode overlay: round counter and scores while a match runs, and the
//! intermission banner (round winner, final standings). Nothing is drawn on a
//! free-for-all server, which has no [`MatchStatus`].

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use shared::{MatchPhase, MatchStatus};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;
use crate::theme::Theme;

// Scores are drawn translucent like the rest of the HUD.
const HUD_ALPHA: f32 = 0.7;
/// Scores shown in the top bar; the final standings list everybody.
const TOP_SCORES: usize = 5;

/// "ROUND 2 / 5", with the clock when a limit runs: "ROUND 2 / 5   1:12".
pub fn header(status: &MatchStatus) -> String {
    let clock = status
        .secs_left
        .map(|secs| format!("   {}:{:02}", secs / 60, secs % 60))
        .unwrap_or_default();
    format!("ROUND {} / {}{clock}", status.round.max(1), status.rounds)
}

/// "name 2   other 1": the best scores, for the top bar.
pub fn scoreline(status: &MatchStatus) -> String {
    status
        .standings
        .iter()
        .take(TOP_SCORES)
        .map(|(name, wins)| format!("{name} {wins}"))
        .collect::<Vec<_>>()
        .join("   ")
}

/// Banner title and subtitle for the phases that interrupt play, `None` while
/// a round is being played or the server is free.
pub fn banner(status: &MatchStatus) -> Option<(String, String)> {
    let next_in = |secs: Option<u16>| secs.map_or(String::new(), |s| format!("Next round in {s}"));
    match status.phase {
        MatchPhase::Free | MatchPhase::Playing => None,
        MatchPhase::Starting => Some((
            format!("MATCH STARTS - {} ROUNDS", status.rounds),
            status
                .secs_left
                .map_or(String::new(), |s| format!("Round 1 in {s}")),
        )),
        MatchPhase::Intermission => Some((winner_line(status, "ROUND"), next_in(status.secs_left))),
        MatchPhase::MatchOver => Some((
            winner_line(status, "ROUND"),
            status.secs_left.map_or("Final standings".to_owned(), |s| {
                format!("Final standings - new match in {s}")
            }),
        )),
    }
}

fn winner_line(status: &MatchStatus, what: &str) -> String {
    match &status.winner {
        Some(name) => format!("{what} {} WON BY {}", status.round, name.to_uppercase()),
        None => format!("{what} {} - NO WINNER", status.round),
    }
}

/// Whether the dead player waits for the next round (rounds mode, a match
/// running) rather than respawning with Space.
pub fn waits_for_round(status: Option<&MatchStatus>) -> bool {
    status.is_some_and(|s| s.phase != MatchPhase::Free)
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
    egui::Area::new("round_bar".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, 12.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                let (head, scores) = match status.phase {
                    MatchPhase::Free => ("WAITING FOR OPPONENTS".to_owned(), String::new()),
                    _ => (header(&status), scoreline(&status)),
                };
                ui.label(
                    egui::RichText::new(head)
                        .size(16.0)
                        .strong()
                        .color(theme.body.egui().gamma_multiply(HUD_ALPHA)),
                );
                if !scores.is_empty() {
                    ui.label(
                        egui::RichText::new(scores)
                            .size(13.0)
                            .color(theme.dim().gamma_multiply(HUD_ALPHA)),
                    );
                }
            });
        });
    let Some((title, subtitle)) = banner(&status) else {
        return;
    };
    egui::Area::new("round_banner".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
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
                    if status.phase == MatchPhase::MatchOver {
                        ui.add_space(8.0);
                        for (place, (name, wins)) in status.standings.iter().enumerate() {
                            ui.label(
                                egui::RichText::new(format!("{}. {name}  -  {wins}", place + 1))
                                    .size(16.0)
                                    .color(theme.body.egui()),
                            );
                        }
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(phase: MatchPhase, winner: Option<&str>) -> MatchStatus {
        MatchStatus {
            rounds: 5,
            round: 2,
            phase,
            secs_left: Some(72),
            winner: winner.map(str::to_owned),
            standings: vec![("Ann".into(), 2), ("Bob".into(), 0)],
        }
    }

    #[test]
    fn header_and_scores_show_round_clock_and_leaders() {
        let s = status(MatchPhase::Playing, None);
        assert_eq!(header(&s), "ROUND 2 / 5   1:12");
        assert_eq!(scoreline(&s), "Ann 2   Bob 0");
    }

    #[test]
    fn banners_name_the_winner_or_the_draw_and_only_interrupt_play() {
        assert_eq!(banner(&status(MatchPhase::Playing, None)), None);
        assert_eq!(banner(&status(MatchPhase::Free, None)), None);
        let (title, sub) = banner(&status(MatchPhase::Intermission, Some("Ann"))).unwrap();
        assert_eq!(title, "ROUND 2 WON BY ANN");
        assert_eq!(sub, "Next round in 72");
        let (title, _) = banner(&status(MatchPhase::Intermission, None)).unwrap();
        assert_eq!(title, "ROUND 2 - NO WINNER");
        let (title, sub) = banner(&status(MatchPhase::MatchOver, Some("Ann"))).unwrap();
        assert_eq!(title, "ROUND 2 WON BY ANN");
        assert_eq!(sub, "Final standings - new match in 72");
    }

    #[test]
    fn the_dead_wait_only_while_a_match_runs() {
        assert!(!waits_for_round(None));
        assert!(!waits_for_round(Some(&status(MatchPhase::Free, None))));
        assert!(waits_for_round(Some(&status(MatchPhase::Playing, None))));
        assert!(waits_for_round(Some(&status(
            MatchPhase::Intermission,
            None
        ))));
    }
}
