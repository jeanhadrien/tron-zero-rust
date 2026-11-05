//! The client's design tokens: one [`Theme`] drives the egui overlays (menu,
//! HUD, death prompt) *and* the world (clear color, grid, walls, line width).
//!
//! Themes are plain data. [`ThemeId`] is what gets saved in the preferences;
//! [`Theme::from_id`] turns it into the full token set. Adding a theme means
//! adding a variant and one constructor here, nothing else.
//!
//! Rider and trail colors are not tokens: the server assigns them
//! (`PlayerColor`) and they must stay recognizable whatever the theme.

use bevy::prelude::{Color, DetectChanges, Local, Res, Resource};
use bevy_egui::{EguiContexts, egui};
use serde::{Deserialize, Serialize};

// Inclusive upper bounds of the rubber warning bands (fraction of base).
pub const RUBBER_RED_MAX: f32 = 0.2;
pub const RUBBER_AMBER_MAX: f32 = 0.5;

/// Opacity of dialog and card fills (out of 255).
const PANEL_ALPHA: u8 = 245;
/// Opacity of the fullscreen backdrop behind the menu (out of 255).
const DIM_ALPHA: u8 = 184;

/// An sRGB color usable by both Bevy (world) and egui (UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn egui(self) -> egui::Color32 {
        egui::Color32::from_rgb(self.0, self.1, self.2)
    }

    pub fn egui_alpha(self, alpha: u8) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(self.0, self.1, self.2, alpha)
    }

    pub fn bevy(self) -> Color {
        Color::srgb_u8(self.0, self.1, self.2)
    }
}

/// Which built-in theme is selected; this is what the preferences store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeId {
    /// The original look: rounded teal panels on a grey backdrop.
    #[default]
    Classic,
    /// 1982: black void, thin pale-cyan vectors, square corners.
    Grid82,
    /// Purple night, magenta walls, yellow accents.
    Arcade,
}

impl ThemeId {
    pub const ALL: [ThemeId; 3] = [ThemeId::Classic, ThemeId::Grid82, ThemeId::Arcade];

    pub fn label(self) -> &'static str {
        match self {
            ThemeId::Classic => "Classic",
            ThemeId::Grid82 => "Grid '82",
            ThemeId::Arcade => "Arcade",
        }
    }
}

/// Every color and metric the client draws with.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Theme {
    pub id: ThemeId,

    // World.
    pub background: Rgb,
    pub grid: Rgb,
    pub wall: Rgb,
    pub dead_trail: Rgb,
    pub dead_rider: Rgb,
    /// Width in pixels of every gizmo line (trails, riders, walls).
    pub line_width: f32,
    /// How many particles each effect makes (1.0 = baseline).
    pub fx_scale: f32,

    // UI.
    /// Headings, key figures, the player's own name.
    pub accent: Rgb,
    /// Secondary text.
    pub body: Rgb,
    /// Text on buttons and other widgets.
    pub text: Rgb,
    pub panel_fill: Rgb,
    pub panel_edge: Rgb,
    pub dim: Rgb,
    pub button: Rgb,
    pub button_hover: Rgb,
    pub button_down: Rgb,
    /// Empty part of a meter, text-field background.
    pub track: Rgb,
    pub danger: Rgb,
    pub warn: Rgb,
    pub panel_radius: u8,
    pub card_radius: u8,
    pub widget_radius: u8,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from_id(ThemeId::default())
    }
}

impl Theme {
    pub fn from_id(id: ThemeId) -> Self {
        match id {
            ThemeId::Classic => Self::classic(),
            ThemeId::Grid82 => Self::grid82(),
            ThemeId::Arcade => Self::arcade(),
        }
    }

    fn classic() -> Self {
        Self {
            id: ThemeId::Classic,
            background: Rgb(51, 51, 51),
            grid: Rgb(15, 31, 38),
            wall: Rgb(115, 128, 140),
            dead_trail: Rgb(77, 77, 77),
            dead_rider: Rgb(255, 38, 38),
            line_width: 2.0,
            fx_scale: 1.0,
            accent: Rgb(0, 255, 204),
            body: Rgb(165, 180, 195),
            text: Rgb(180, 190, 200),
            panel_fill: Rgb(8, 15, 23),
            panel_edge: Rgb(20, 60, 66),
            dim: Rgb(3, 5, 10),
            button: Rgb(15, 56, 66),
            button_hover: Rgb(20, 87, 97),
            button_down: Rgb(26, 128, 122),
            track: Rgb(31, 46, 56),
            danger: Rgb(255, 77, 77),
            warn: Rgb(255, 191, 64),
            panel_radius: 12,
            card_radius: 8,
            widget_radius: 8,
        }
    }

    fn grid82() -> Self {
        Self {
            id: ThemeId::Grid82,
            background: Rgb(0, 0, 0),
            grid: Rgb(6, 38, 58),
            wall: Rgb(140, 235, 255),
            dead_trail: Rgb(55, 65, 75),
            dead_rider: Rgb(255, 60, 40),
            line_width: 2.5,
            fx_scale: 0.8,
            accent: Rgb(120, 232, 255),
            body: Rgb(140, 180, 200),
            text: Rgb(210, 240, 255),
            panel_fill: Rgb(0, 6, 12),
            panel_edge: Rgb(0, 130, 160),
            dim: Rgb(0, 0, 0),
            button: Rgb(0, 28, 42),
            button_hover: Rgb(0, 68, 92),
            button_down: Rgb(0, 112, 142),
            track: Rgb(8, 24, 34),
            danger: Rgb(255, 60, 40),
            warn: Rgb(255, 170, 0),
            panel_radius: 0,
            card_radius: 0,
            widget_radius: 0,
        }
    }

    fn arcade() -> Self {
        Self {
            id: ThemeId::Arcade,
            background: Rgb(14, 6, 28),
            grid: Rgb(44, 20, 80),
            wall: Rgb(255, 64, 200),
            dead_trail: Rgb(80, 60, 100),
            dead_rider: Rgb(255, 60, 80),
            line_width: 3.0,
            fx_scale: 1.6,
            accent: Rgb(255, 230, 0),
            body: Rgb(190, 170, 230),
            text: Rgb(245, 235, 255),
            panel_fill: Rgb(20, 8, 40),
            panel_edge: Rgb(255, 64, 200),
            dim: Rgb(8, 2, 16),
            button: Rgb(60, 20, 90),
            button_hover: Rgb(100, 30, 130),
            button_down: Rgb(150, 40, 170),
            track: Rgb(36, 16, 60),
            danger: Rgb(255, 60, 80),
            warn: Rgb(255, 160, 0),
            panel_radius: 0,
            card_radius: 0,
            widget_radius: 0,
        }
    }

    /// Fullscreen backdrop behind the menu.
    pub fn dim(&self) -> egui::Color32 {
        self.dim.egui_alpha(DIM_ALPHA)
    }

    pub fn panel(&self) -> egui::Color32 {
        self.panel_fill.egui_alpha(PANEL_ALPHA)
    }

    /// Warning color for a rubber fraction in [0, 1]: HUD fill and numbers.
    pub fn rubber_color(&self, fraction: f32) -> egui::Color32 {
        if fraction <= RUBBER_RED_MAX {
            self.danger.egui()
        } else if fraction <= RUBBER_AMBER_MAX {
            self.warn.egui()
        } else {
            self.accent.egui()
        }
    }

    fn frame(&self, radius: u8, margin: i8) -> egui::Frame {
        egui::Frame::default()
            .fill(self.panel())
            .stroke(egui::Stroke::new(1.0, self.panel_edge.egui()))
            .corner_radius(egui::CornerRadius::same(radius))
            .inner_margin(egui::Margin::same(margin))
    }

    /// Centered dialog frame (menu + death prompt).
    pub fn panel_frame(&self) -> egui::Frame {
        self.frame(self.panel_radius, 28)
    }

    /// Smaller sibling for the HUD meter cards.
    pub fn card_frame(&self) -> egui::Frame {
        self.frame(self.card_radius, 16)
    }

    /// Make this theme the egui-wide style: every widget, in every overlay,
    /// picks its colors and corners from here without per-panel overrides.
    pub fn apply_to_egui(&self, ctx: &egui::Context) {
        let radius = egui::CornerRadius::same(self.widget_radius);
        let edge = egui::Stroke::new(1.0, self.panel_edge.egui());
        let mut visuals = egui::Visuals::dark();

        visuals.window_fill = self.panel();
        visuals.panel_fill = self.panel();
        visuals.window_stroke = edge;
        visuals.window_corner_radius = egui::CornerRadius::same(self.panel_radius);
        visuals.menu_corner_radius = radius;
        visuals.window_shadow = egui::Shadow::NONE;
        visuals.popup_shadow = egui::Shadow::NONE;
        visuals.extreme_bg_color = self.track.egui();
        visuals.faint_bg_color = self.track.egui();
        visuals.hyperlink_color = self.accent.egui();
        visuals.warn_fg_color = self.warn.egui();
        visuals.error_fg_color = self.danger.egui();
        visuals.selection.bg_fill = self.button_down.egui();
        visuals.selection.stroke = egui::Stroke::new(1.0, self.accent.egui());

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.panel();
        widgets.noninteractive.bg_stroke = edge;
        widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, self.body.egui());
        widgets.noninteractive.corner_radius = radius;
        for (state, fill, fg) in [
            (&mut widgets.inactive, self.button, self.text),
            (&mut widgets.hovered, self.button_hover, self.accent),
            (&mut widgets.active, self.button_down, self.text),
            (&mut widgets.open, self.button, self.text),
        ] {
            state.bg_fill = fill.egui();
            state.weak_bg_fill = fill.egui();
            state.bg_stroke = egui::Stroke::NONE;
            state.fg_stroke = egui::Stroke::new(1.0, fg.egui());
            state.corner_radius = radius;
            state.expansion = 0.0;
        }
        // A hovered button keeps a visible outline: with square corners the
        // fill change alone is easy to miss.
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0, self.accent.egui());

        ctx.set_visuals(visuals);
    }
}

/// Keep egui's global style in sync with the theme. Runs before the overlays;
/// retries each frame until egui has a context (the first frame has none).
pub fn apply_egui_theme(mut contexts: EguiContexts, theme: Res<Theme>, mut applied: Local<bool>) {
    if *applied && !theme.is_changed() {
        return;
    }
    if let Ok(ctx) = contexts.ctx_mut() {
        theme.apply_to_egui(ctx);
        *applied = true;
    }
}

/// Airy rows for the dialog panels (menu); colors come from the global style.
pub fn panel_spacing(ui: &mut egui::Ui) {
    ui.style_mut().spacing.item_spacing = egui::vec2(8.0, 14.0);
}

/// Full-width 44px action button (the old menu's uniform buttons).
pub fn action_button(ui: &mut egui::Ui, caption: &str) -> egui::Response {
    let width = ui.available_width();
    ui.add_sized(
        [width, 44.0],
        egui::Button::new(egui::RichText::new(caption).size(20.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_id_builds_its_own_theme() {
        for id in ThemeId::ALL {
            assert_eq!(Theme::from_id(id).id, id);
        }
    }

    #[test]
    fn rubber_warning_thresholds_are_inclusive() {
        let theme = Theme::default();
        assert_eq!(theme.rubber_color(0.51), theme.accent.egui());
        assert_eq!(theme.rubber_color(0.5), theme.warn.egui());
        assert_eq!(theme.rubber_color(0.21), theme.warn.egui());
        assert_eq!(theme.rubber_color(0.2), theme.danger.egui());
        assert_eq!(theme.rubber_color(0.0), theme.danger.egui());
    }

    #[test]
    fn egui_style_comes_from_the_theme() {
        for id in ThemeId::ALL {
            let theme = Theme::from_id(id);
            let ctx = egui::Context::default();
            theme.apply_to_egui(&ctx);
            let visuals = ctx.global_style().visuals.clone();
            assert_eq!(visuals.widgets.inactive.bg_fill, theme.button.egui());
            assert_eq!(visuals.widgets.hovered.bg_fill, theme.button_hover.egui());
            assert_eq!(visuals.window_fill, theme.panel());
            assert_eq!(
                visuals.widgets.inactive.corner_radius,
                egui::CornerRadius::same(theme.widget_radius)
            );
        }
    }

    #[test]
    fn theme_id_round_trips_through_json() {
        for id in ThemeId::ALL {
            let text = serde_json::to_string(&id).unwrap();
            assert_eq!(serde_json::from_str::<ThemeId>(&text).unwrap(), id);
        }
    }
}
