//! Shared palette for the egui overlays (menu, HUD, death prompt).
//!
//! Every overlay reads its colors from here so the look can never drift
//! apart. Colors are exposed in both `egui::Color32` (drawing) and
//! `bevy::prelude::Color` (tests / gizmos) forms; the two are matched
//! channel-by-channel from the same 0-255 source values.

use bevy::prelude::Color;
use bevy_egui::egui;

// Inclusive upper bounds of the rubber warning bands (fraction of base).
pub const RUBBER_RED_MAX: f32 = 0.2;
pub const RUBBER_AMBER_MAX: f32 = 0.5;

// --- egui forms (used for drawing) ---

pub const CYAN: egui::Color32 = egui::Color32::from_rgb(0, 255, 204);
pub const BODY: egui::Color32 = egui::Color32::from_rgb(165, 180, 195);
pub const PANEL_EDGE: egui::Color32 = egui::Color32::from_rgb(20, 60, 66);
pub const BTN: egui::Color32 = egui::Color32::from_rgb(15, 56, 66);
pub const BTN_HOVER: egui::Color32 = egui::Color32::from_rgb(20, 87, 97);
pub const BTN_DOWN: egui::Color32 = egui::Color32::from_rgb(26, 128, 122);
pub const RUBBER_RED: egui::Color32 = egui::Color32::from_rgb(255, 77, 77);
pub const RUBBER_AMBER: egui::Color32 = egui::Color32::from_rgb(255, 191, 64);
pub const TRACK: egui::Color32 = egui::Color32::from_rgb(31, 46, 56);

// Alpha-blended fills can't be `const` (non-const constructor), so helpers.
pub fn dim() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(3, 5, 10, 184)
}

pub fn panel() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(8, 15, 23, 245)
}

// --- Bevy forms (same channels, for tests / non-egui use) ---
// Only exercised by unit tests (`cargo check` skips those), hence the allow.
#[allow(dead_code)]
pub const CYAN_BEVY: Color = Color::srgb_u8(0, 255, 204);
#[allow(dead_code)]
pub const BODY_BEVY: Color = Color::srgb_u8(165, 180, 195);
#[allow(dead_code)]
pub const PANEL_EDGE_BEVY: Color = Color::srgb_u8(20, 60, 66);
#[allow(dead_code)]
pub const BTN_BEVY: Color = Color::srgb_u8(15, 56, 66);
#[allow(dead_code)]
pub const BTN_HOVER_BEVY: Color = Color::srgb_u8(20, 87, 97);
#[allow(dead_code)]
pub const BTN_DOWN_BEVY: Color = Color::srgb_u8(26, 128, 122);
#[allow(dead_code)]
pub const RUBBER_RED_BEVY: Color = Color::srgb_u8(255, 77, 77);
#[allow(dead_code)]
pub const RUBBER_AMBER_BEVY: Color = Color::srgb_u8(255, 191, 64);
#[allow(dead_code)]
pub const TRACK_BEVY: Color = Color::srgb(0.12, 0.18, 0.22);

#[allow(dead_code)]
pub fn dim_bevy() -> Color {
    Color::srgba_u8(3, 5, 10, 184)
}

#[allow(dead_code)]
pub fn panel_bevy() -> Color {
    Color::srgba_u8(8, 15, 23, 245)
}

// Warning color for a rubber fraction in [0, 1].
#[allow(dead_code)]
pub fn rubber_bevy(fraction: f32) -> Color {
    if fraction <= RUBBER_RED_MAX {
        RUBBER_RED_BEVY
    } else if fraction <= RUBBER_AMBER_MAX {
        RUBBER_AMBER_BEVY
    } else {
        CYAN_BEVY
    }
}

// egui twin of [`rubber_bevy`], used for the HUD fill and numbers.
pub fn rubber_egui(fraction: f32) -> egui::Color32 {
    if fraction <= RUBBER_RED_MAX {
        RUBBER_RED
    } else if fraction <= RUBBER_AMBER_MAX {
        RUBBER_AMBER
    } else {
        CYAN
    }
}

// Centered dialog frame (menu + death prompt).
pub fn panel_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(panel())
        .stroke(egui::Stroke::new(1.0, PANEL_EDGE))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(28))
}

// Smaller sibling for the HUD meter cards.
pub fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(panel())
        .stroke(egui::Stroke::new(1.0, PANEL_EDGE))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(16))
}

// Panel-local styling: airy rows, teal buttons with rounded corners.
pub fn theme_panel(ui: &mut egui::Ui) {
    ui.style_mut().spacing.item_spacing = egui::vec2(8.0, 14.0);
    let visuals = ui.visuals_mut();
    visuals.widgets.inactive.bg_fill = BTN;
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
    visuals.widgets.hovered.bg_fill = BTN_HOVER;
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
    visuals.widgets.active.bg_fill = BTN_DOWN;
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(8);
}

// Full-width 44px action button (the old menu's uniform buttons).
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

    fn channels_bevy(c: Color) -> [u8; 4] {
        let s = c.to_srgba();
        [
            (s.red * 255.0).round() as u8,
            (s.green * 255.0).round() as u8,
            (s.blue * 255.0).round() as u8,
            (s.alpha * 255.0).round() as u8,
        ]
    }

    #[test]
    fn egui_and_bevy_palettes_agree() {
        fn channels_egui(c: egui::Color32) -> [u8; 4] {
            [c.r(), c.g(), c.b(), c.a()]
        }
        for (e, b) in [
            (CYAN, CYAN_BEVY),
            (BODY, BODY_BEVY),
            (PANEL_EDGE, PANEL_EDGE_BEVY),
            (BTN, BTN_BEVY),
            (BTN_HOVER, BTN_HOVER_BEVY),
            (BTN_DOWN, BTN_DOWN_BEVY),
            (RUBBER_RED, RUBBER_RED_BEVY),
            (RUBBER_AMBER, RUBBER_AMBER_BEVY),
        ] {
            assert_eq!(channels_egui(e), channels_bevy(b));
        }
        assert_eq!(channels_egui(dim()), channels_bevy(dim_bevy()));
        assert_eq!(channels_egui(panel()), channels_bevy(panel_bevy()));
    }

    #[test]
    fn rubber_bands_agree_across_forms() {
        for f in [0.0, 0.2, 0.21, 0.5, 0.51, 1.0] {
            let e = rubber_egui(f);
            let [r, g, b, _] = channels_bevy(rubber_bevy(f));
            assert_eq!([e.r(), e.g(), e.b()], [r, g, b], "fraction {f}");
        }
    }
}
