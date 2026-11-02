//! Shared palette for the egui overlays (menu, HUD, death prompt).
//!
//! Every overlay reads its colors from here so the look can never drift apart.

use bevy_egui::egui;

// Inclusive upper bounds of the rubber warning bands (fraction of base).
pub const RUBBER_RED_MAX: f32 = 0.2;
pub const RUBBER_AMBER_MAX: f32 = 0.5;

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

// Warning color for a rubber fraction in [0, 1]: HUD fill and numbers.
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
