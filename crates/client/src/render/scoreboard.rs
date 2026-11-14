//! Scoreboard overlay: every rider's match record (kills, deaths, round wins)
//! and humans' ping, as the server replicates them. Shown while Tab is held,
//! and on its own on the end screen of a match.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lightyear::prelude::Predicted;
use shared::{DisplayName, MatchPhase, MatchStatus, Player, RoundTrip, Score};

use crate::chat::ChatState;
use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;
use crate::theme::Theme;

/// Key held to show the scoreboard.
const KEY: KeyCode = KeyCode::Tab;

/// Whether the scoreboard is up this frame; the death panel stays out of its way.
#[derive(Resource, Default)]
pub struct ScoreboardShown(pub bool);

/// One line of the scoreboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub score: Score,
    /// `None` for a bot.
    pub ping: Option<u16>,
    /// The local player's own rider.
    pub me: bool,
}

/// Best first: round wins, then kills, then fewest deaths, then name.
pub fn sort(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        (b.score.wins, b.score.kills, a.score.deaths, &a.name).cmp(&(
            a.score.wins,
            a.score.kills,
            b.score.deaths,
            &b.name,
        ))
    });
}

/// Up while in a game with the menu and the chat box closed, when the key is
/// held or the match is over.
pub fn visible(
    phase: ConnectionPhase,
    menu_open: bool,
    typing: bool,
    held: bool,
    status: Option<&MatchStatus>,
) -> bool {
    let over = status.is_some_and(|s| s.phase == MatchPhase::MatchOver);
    phase == ConnectionPhase::Playing && !menu_open && !typing && (held || over)
}

pub fn scoreboard_ui(
    mut contexts: EguiContexts,
    session: Res<Session>,
    menu: Res<MenuState>,
    chat: Res<ChatState>,
    keys: Res<ButtonInput<KeyCode>>,
    theme: Res<Theme>,
    status: Option<Single<&MatchStatus>>,
    riders: Query<
        (
            &DisplayName,
            Option<&Score>,
            Option<&RoundTrip>,
            Has<Predicted>,
        ),
        With<Player>,
    >,
    mut shown: ResMut<ScoreboardShown>,
) {
    let status = status.as_deref().copied();
    shown.0 = visible(
        session.phase(),
        menu.open,
        chat.typing,
        keys.pressed(KEY),
        status,
    );
    if !shown.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut rows: Vec<Row> = riders
        .iter()
        .map(|(name, score, ping, me)| Row {
            name: name.0.clone(),
            score: score.copied().unwrap_or_default(),
            // No sample yet reads 0: show nothing, like the HUD.
            ping: ping.map(|p| p.0).filter(|ms| *ms > 0),
            me,
        })
        .collect();
    sort(&mut rows);
    let wins = status.is_some_and(|s| s.rounds > 1);

    egui::Area::new("scoreboard".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .interactable(false)
        .show(ctx, |ui| {
            theme.panel_frame().show(ui, |ui| {
                ui.set_min_width(384.0);
                egui::Grid::new("scoreboard_rows")
                    .num_columns(if wins { 5 } else { 4 })
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        let head = |ui: &mut egui::Ui, text: &str| {
                            ui.label(egui::RichText::new(text).size(14.0).color(theme.dim()));
                        };
                        head(ui, "PLAYER");
                        head(ui, "KILLS");
                        head(ui, "DEATHS");
                        if wins {
                            head(ui, "WINS");
                        }
                        head(ui, "PING");
                        ui.end_row();
                        for row in &rows {
                            let color = if row.me {
                                theme.accent.egui()
                            } else {
                                theme.body.egui()
                            };
                            let cell = |ui: &mut egui::Ui, text: String| {
                                let text = egui::RichText::new(text).size(16.0).color(color);
                                ui.label(if row.me { text.strong() } else { text });
                            };
                            cell(ui, row.name.clone());
                            cell(ui, row.score.kills.to_string());
                            cell(ui, row.score.deaths.to_string());
                            if wins {
                                cell(ui, row.score.wins.to_string());
                            }
                            cell(ui, row.ping.map_or(String::new(), |ms| format!("{ms} ms")));
                            ui.end_row();
                        }
                    });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, wins: u32, kills: u32, deaths: u32) -> Row {
        Row {
            name: name.to_owned(),
            score: Score {
                kills,
                deaths,
                wins,
            },
            ping: None,
            me: false,
        }
    }

    #[test]
    fn rows_rank_by_wins_then_kills_then_fewest_deaths_then_name() {
        let mut rows = vec![
            row("Eve", 0, 9, 0),
            row("Bob", 1, 2, 5),
            row("Cid", 1, 2, 3),
            row("Ann", 1, 2, 3),
            row("Dan", 1, 4, 9),
        ];
        sort(&mut rows);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Dan", "Ann", "Cid", "Bob", "Eve"]);
    }

    #[test]
    fn the_board_shows_while_held_or_on_the_end_screen_but_not_over_the_menu_or_chat() {
        let over = MatchStatus {
            phase: MatchPhase::MatchOver,
            ..MatchStatus::default()
        };
        let playing = MatchStatus {
            phase: MatchPhase::Playing,
            ..MatchStatus::default()
        };
        let live = ConnectionPhase::Playing;
        assert!(visible(live, false, false, true, Some(&playing)));
        assert!(!visible(live, false, false, false, Some(&playing)));
        assert!(visible(live, false, false, false, Some(&over)));
        assert!(!visible(live, true, false, true, Some(&over)));
        assert!(!visible(live, false, true, true, Some(&playing)));
        assert!(!visible(
            ConnectionPhase::Connecting,
            false,
            false,
            true,
            None
        ));
    }
}
