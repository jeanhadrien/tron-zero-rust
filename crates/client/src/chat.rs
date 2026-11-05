//! Player chat: the typing state machine, the received-line log and the
//! top-left overlay.
//!
//! Everything is event-driven. `chat_controls` only acts on keyboard events,
//! `receive_chat` only on arriving messages, and the overlay is a pure function
//! of the log. Text comes from raw `KeyboardInput` messages (the same source as
//! the turn keys), so there is no egui focus to race against: while
//! [`ChatState::captures_input`] is true the turn keys are ignored entirely.

use std::collections::VecDeque;

use bevy::ecs::message::MessageReader;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lightyear::prelude::{Client, Connected, MessageReceiver, MessageSender};
use shared::protocol::{
    ChatChannel, ChatLine, ChatSend, MAX_CHAT_CHARS, is_chat_char, sanitize_chat,
};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;
use crate::theme::Theme;

/// Lines kept in memory.
const LOG_CAP: usize = 50;
/// Lines shown while typing (the log is the history, the screen stays small).
const SHOWN_WHILE_TYPING: usize = 20;
/// Lines shown while idle (only fresh ones).
const SHOWN_WHILE_IDLE: usize = 8;
/// A line stays fully visible this long, then fades out over [`FADE_SECS`].
const VISIBLE_SECS: f64 = 8.0;
const FADE_SECS: f64 = 2.0;
/// Wrap width of the overlay, in points.
const WRAP_WIDTH: f32 = 520.0;

/// Whether the player is composing a message, and what they typed so far.
#[derive(Resource, Default)]
pub struct ChatState {
    pub typing: bool,
    pub buffer: String,
    /// A chat key was handled this frame; the turn keys must ignore it (the
    /// frame that closes the box would otherwise leak its keys into the game).
    swallow_this_frame: bool,
}

impl ChatState {
    pub fn captures_input(&self) -> bool {
        self.typing || self.swallow_this_frame
    }

    fn close(&mut self) {
        self.typing = false;
        self.buffer.clear();
    }
}

/// One received line and when it arrived (`Time<Real>` seconds).
pub struct ChatEntry {
    pub name: String,
    pub text: String,
    pub at: f64,
}

#[derive(Resource, Default)]
pub struct ChatLog(pub VecDeque<ChatEntry>);

impl ChatLog {
    fn push(&mut self, entry: ChatEntry) {
        if self.0.len() == LOG_CAP {
            self.0.pop_front();
        }
        self.0.push_back(entry);
    }
}

/// Opacity of a line of the given age: 1 while fresh, then fading to 0.
pub fn line_alpha(age: f64) -> f32 {
    (1.0 - (age - VISIBLE_SECS) / FADE_SECS).clamp(0.0, 1.0) as f32
}

/// The lines to draw, oldest first, with their opacity: the recent history
/// while typing, otherwise only the lines that have not faded away.
pub fn visible_lines(log: &ChatLog, now: f64, typing: bool) -> Vec<(&ChatEntry, f32)> {
    let mut lines: Vec<_> = log
        .0
        .iter()
        .rev()
        .map(|entry| {
            let alpha = if typing {
                1.0
            } else {
                line_alpha(now - entry.at)
            };
            (entry, alpha)
        })
        .filter(|(_, alpha)| *alpha > 0.0)
        .take(if typing {
            SHOWN_WHILE_TYPING
        } else {
            SHOWN_WHILE_IDLE
        })
        .collect();
    lines.reverse();
    lines
}

/// Append a typed character unless it is not allowed or the box is full.
fn push_char(buffer: &mut String, c: char) {
    if is_chat_char(c) && buffer.chars().count() < MAX_CHAT_CHARS {
        buffer.push(c);
    }
}

/// Keyboard side of the chat: Enter opens the box (and sends when open),
/// Escape cancels, Backspace erases and printable text is appended.
///
/// Runs in `PreUpdate` after `menu_controls` (which leaves Escape alone while
/// the box is open) and before the turn keys are read.
#[allow(clippy::type_complexity)]
pub fn chat_controls(
    mut key_events: MessageReader<KeyboardInput>,
    mut chat: ResMut<ChatState>,
    mut log: ResMut<ChatLog>,
    session: Res<Session>,
    menu: Res<MenuState>,
    mut senders: Query<&mut MessageSender<ChatSend>, (With<Client>, With<Connected>)>,
) {
    chat.swallow_this_frame = false;
    if session.phase != ConnectionPhase::Playing || menu.open {
        chat.close();
        if matches!(
            session.phase,
            ConnectionPhase::Offline | ConnectionPhase::Connecting
        ) {
            log.0.clear();
        }
        key_events.clear();
        return;
    }
    for event in key_events.read() {
        if !event.state.is_pressed() {
            continue;
        }
        if !chat.typing {
            if event.logical_key == Key::Enter && !event.repeat && !menu.block_this_frame {
                chat.typing = true;
                chat.swallow_this_frame = true;
            }
            continue;
        }
        chat.swallow_this_frame = true;
        match &event.logical_key {
            Key::Escape => chat.close(),
            Key::Enter => {
                if let Some(text) = sanitize_chat(&chat.buffer) {
                    for mut sender in &mut senders {
                        sender.send::<ChatChannel>(ChatSend { text: text.clone() });
                    }
                }
                chat.close();
            }
            Key::Backspace => {
                chat.buffer.pop();
            }
            _ => {
                if let Some(text) = &event.text {
                    for c in text.chars() {
                        push_char(&mut chat.buffer, c);
                    }
                }
            }
        }
    }
}

/// Move arriving chat lines into the log.
pub fn receive_chat(
    time: Res<Time<Real>>,
    mut clients: Query<&mut MessageReceiver<ChatLine>, (With<Client>, With<Connected>)>,
    mut log: ResMut<ChatLog>,
) {
    for mut receiver in &mut clients {
        for line in receiver.receive() {
            log.push(ChatEntry {
                name: line.name,
                text: line.text,
                at: time.elapsed_secs_f64(),
            });
        }
    }
}

/// Top-left chat overlay: no background, a one-pixel shadow keeps the text
/// legible over the arena. Non-interactive, so clicks fall through.
pub fn chat_ui(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    session: Res<Session>,
    menu: Res<MenuState>,
    theme: Res<Theme>,
    chat: Res<ChatState>,
    log: Res<ChatLog>,
) {
    if session.phase != ConnectionPhase::Playing || menu.open {
        return;
    }
    let lines = visible_lines(&log, time.elapsed_secs_f64(), chat.typing);
    if lines.is_empty() && !chat.typing {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Area::new("chat".into())
        .anchor(egui::Align2::LEFT_TOP, [16.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_max_width(WRAP_WIDTH);
            for (entry, alpha) in lines {
                let name = format!("{}: ", entry.name);
                shadowed_line(
                    ui,
                    (&name, theme.accent.egui().gamma_multiply(alpha)),
                    (&entry.text, theme.body.egui().gamma_multiply(alpha)),
                );
            }
            if chat.typing {
                let prompt = format!("{}: ", session.name);
                let typed = format!("{}|", chat.buffer);
                shadowed_line(
                    ui,
                    (&prompt, theme.accent.egui()),
                    (&typed, theme.body.egui()),
                );
            }
        });
}

/// One wrapped line made of a coloured head and tail, drawn twice (shadow,
/// then text) because egui has no text shadow.
fn shadowed_line(ui: &mut egui::Ui, head: (&str, egui::Color32), tail: (&str, egui::Color32)) {
    let font = egui::FontId::proportional(16.0);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = WRAP_WIDTH;
    for (text, color) in [head, tail] {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    }
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let shadow = egui::Color32::from_black_alpha(200);
    ui.painter().galley_with_override_text_color(
        rect.min + egui::vec2(1.0, 1.0),
        galley.clone(),
        shadow,
    );
    ui.painter().galley(rect.min, galley, egui::Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_with(arrivals: &[f64]) -> ChatLog {
        let mut log = ChatLog::default();
        for (i, at) in arrivals.iter().enumerate() {
            log.push(ChatEntry {
                name: "a".into(),
                text: i.to_string(),
                at: *at,
            });
        }
        log
    }

    #[test]
    fn lines_fade_after_a_while() {
        assert_eq!(line_alpha(0.0), 1.0);
        assert_eq!(line_alpha(VISIBLE_SECS), 1.0);
        assert!((line_alpha(VISIBLE_SECS + FADE_SECS / 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(line_alpha(VISIBLE_SECS + FADE_SECS), 0.0);
    }

    #[test]
    fn idle_view_hides_old_lines_and_typing_view_shows_history() {
        let log = log_with(&[0.0, 95.0, 99.0]);
        let idle: Vec<_> = visible_lines(&log, 100.0, false)
            .iter()
            .map(|(entry, _)| entry.text.clone())
            .collect();
        assert_eq!(idle, ["1", "2"]);
        assert_eq!(visible_lines(&log, 100.0, true).len(), 3);
    }

    #[test]
    fn log_is_capped() {
        let log = log_with(&vec![0.0; LOG_CAP + 10]);
        assert_eq!(log.0.len(), LOG_CAP);
        assert_eq!(log.0.front().unwrap().text, "10");
    }

    #[test]
    fn typed_text_is_filtered_and_bounded() {
        let mut buffer = String::new();
        for c in "héllo\u{202E}\u{7} wörld ù$".chars() {
            push_char(&mut buffer, c);
        }
        assert_eq!(buffer, "héllo wörld ù$");
        for _ in 0..MAX_CHAT_CHARS + 5 {
            push_char(&mut buffer, 'x');
        }
        assert_eq!(buffer.chars().count(), MAX_CHAT_CHARS);
    }
}
