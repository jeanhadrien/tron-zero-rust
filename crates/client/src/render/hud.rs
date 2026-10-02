//! Bottom-anchored rubber + speed meters drawn with egui.
//!
//! Values come from the local predicted rider; visibility matches the old
//! Bevy-UI HUD (Playing, menu closed, local rider alive).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, Controlled, ControlledBy, Predicted};
use shared::{BASE_RUBBER, IsAlive, Player, PlayerInput, Rubber, SpeedMult};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;
use crate::theme;

// Fixed card width (matches the old 220px Bevy-UI cards). Without the max,
// the Area auto-sizes to content and `available_width()` below is unbounded,
// which stretched the rubber bar across the screen.
const CARD_WIDTH: f32 = 220.0;

// Same visibility rule as the old retained HUD.
pub fn hud_visible(phase: ConnectionPhase, menu_open: bool, alive: Option<bool>) -> bool {
    phase == ConnectionPhase::Playing && !menu_open && alive.is_some_and(|a| a)
}

// Rubber fraction clamped to [0, 1]; pure so tests skip the egui context.
pub fn rubber_fraction(rubber: f32) -> f32 {
    (rubber / BASE_RUBBER).clamp(0.0, 1.0)
}

pub fn rubber_percent(fraction: f32) -> u32 {
    (fraction * 100.0).round() as u32
}

// "1.25x" style with two decimals; matches the old `1.25x` formatting.
pub fn format_speed(speed: f32) -> String {
    let number = (speed.max(0.0) * 100.0).round() as u32;
    format!("{}.{:02}x", number / 100, number % 100)
}

#[allow(clippy::type_complexity)]
pub fn hud_ui(
    mut contexts: EguiContexts,
    session: Res<Session>,
    menu: Res<MenuState>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    players: Query<
        (&Rubber, &SpeedMult, &IsAlive, Option<&ControlledBy>),
        (
            With<Player>,
            With<Controlled>,
            With<Predicted>,
            With<InputMarker<PlayerInput>>,
        ),
    >,
) {
    let local = clients.single().ok().and_then(|client| {
        players
            .iter()
            .find(|(_, _, _, owner)| owner.is_none_or(|owner| owner.owner == client))
    });
    if !hud_visible(
        session.phase,
        menu.open,
        local.map(|(_, _, alive, _)| alive.0),
    ) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Some((rubber, speed, _, _)) = local else {
        return;
    };
    let fraction = rubber_fraction(rubber.0);
    let color = theme::rubber_egui(fraction);

    // Two cards pinned to the bottom corners; non-interactive so gameplay
    // clicks pass through. Painted in the default Middle order — the menu
    // hides the HUD outright when open, so it always wins any tie.
    egui::Area::new("hud_rubber".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [20.0, -20.0])
        .interactable(false)
        .show(ctx, |ui| {
            theme::card_frame().show(ui, |ui| {
                ui.set_min_width(CARD_WIDTH);
                ui.set_max_width(CARD_WIDTH);
                ui.label(egui::RichText::new("RUBBER").size(14.0).color(theme::BODY));
                ui.label(
                    egui::RichText::new(format!("{}%", rubber_percent(fraction)))
                        .size(30.0)
                        .color(color),
                );
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 6.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(rect, 0.0, theme::TRACK);
                let fill = egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2(rect.width() * fraction, rect.height()),
                );
                ui.painter().rect_filled(fill, 0.0, color);
            });
        });
    egui::Area::new("hud_speed".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -20.0])
        .interactable(false)
        .show(ctx, |ui| {
            theme::card_frame().show(ui, |ui| {
                ui.set_min_width(CARD_WIDTH);
                ui.set_max_width(CARD_WIDTH);
                ui.label(egui::RichText::new("SPEED").size(14.0).color(theme::BODY));
                ui.label(
                    egui::RichText::new(format_speed(speed.0))
                        .size(30.0)
                        .color(theme::CYAN),
                );
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{CYAN_BEVY, RUBBER_AMBER_BEVY, RUBBER_RED_BEVY};
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::{PeerId, RemoteId};

    #[test]
    fn rubber_warning_thresholds_are_inclusive() {
        assert_eq!(theme::rubber_bevy(0.51), CYAN_BEVY);
        assert_eq!(theme::rubber_bevy(0.5), RUBBER_AMBER_BEVY);
        assert_eq!(theme::rubber_bevy(0.21), RUBBER_AMBER_BEVY);
        assert_eq!(theme::rubber_bevy(0.2), RUBBER_RED_BEVY);
        assert_eq!(theme::rubber_bevy(0.0), RUBBER_RED_BEVY);
    }

    #[test]
    fn rubber_math_matches_old_meters() {
        assert_eq!(rubber_fraction(BASE_RUBBER / 2.0), 0.5);
        assert_eq!(rubber_percent(0.5), 50);
        assert_eq!(rubber_percent(1.0), 100);
        assert_eq!(format_speed(1.25), "1.25x");
        assert_eq!(format_speed(1.0), "1.00x");
        assert_eq!(format_speed(-3.0), "0.00x");
    }

    #[test]
    fn hud_shows_only_while_playing_with_live_rider() {
        assert!(hud_visible(ConnectionPhase::Playing, false, Some(true)));
        assert!(!hud_visible(ConnectionPhase::Playing, true, Some(true)));
        assert!(!hud_visible(ConnectionPhase::Playing, false, Some(false)));
        assert!(!hud_visible(ConnectionPhase::Playing, false, None));
        assert!(!hud_visible(
            ConnectionPhase::Disconnecting,
            false,
            Some(true)
        ));
    }

    #[test]
    fn meters_follow_local_prediction_and_session_lifecycle() {
        let mut world = World::new();
        world.init_resource::<Session>();
        world.init_resource::<MenuState>();

        // No client yet: nothing visible.
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0);

        world.spawn((Client::default(), RemoteId(PeerId::Server), Connected));
        // Confirmed/remote copies must never supply the HUD values.
        world.spawn((Player, Rubber(1.0), SpeedMult(9.0), IsAlive(true)));
        let rider = world
            .spawn((
                Player,
                Controlled,
                Predicted,
                InputMarker::<PlayerInput>::default(),
                Rubber(BASE_RUBBER / 2.0),
                SpeedMult(1.25),
                IsAlive(true),
            ))
            .id();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.resource_mut::<MenuState>().open = false;
        world.run_system_once(update_probe).unwrap();
        let probe = world.resource::<Probe>();
        assert!(probe.0);
        assert_eq!(probe.1, Some(50));
        assert_eq!(probe.2.as_deref(), Some("1.25x"));

        world.entity_mut(rider).insert(Rubber(0.0));
        world.run_system_once(update_probe).unwrap();
        assert_eq!(world.resource::<Probe>().1, Some(0));

        world.resource_mut::<MenuState>().open = true;
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0);

        world.resource_mut::<MenuState>().open = false;
        world.entity_mut(rider).insert(IsAlive(false));
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0);

        world
            .entity_mut(rider)
            .insert((IsAlive(true), Rubber(BASE_RUBBER), SpeedMult(1.0)));
        world.run_system_once(update_probe).unwrap();
        let probe = world.resource::<Probe>();
        assert!(probe.0);
        assert_eq!(probe.1, Some(100));
        assert_eq!(probe.2.as_deref(), Some("1.00x"));

        world.resource_mut::<Session>().phase = ConnectionPhase::Disconnecting;
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0);

        world.despawn(rider);
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0);
    }

    // Mirrors hud_ui's lookup + visibility without needing an egui context.
    #[derive(Resource, Default)]
    struct Probe(bool, Option<u32>, Option<String>);

    #[allow(clippy::type_complexity)]
    fn update_probe(
        session: Res<Session>,
        menu: Res<MenuState>,
        clients: Query<Entity, (With<Client>, With<Connected>)>,
        players: Query<
            (&Rubber, &SpeedMult, &IsAlive, Option<&ControlledBy>),
            (
                With<Player>,
                With<Controlled>,
                With<Predicted>,
                With<InputMarker<PlayerInput>>,
            ),
        >,
        probe: Option<ResMut<Probe>>,
    ) {
        let local = clients.single().ok().and_then(|client| {
            players
                .iter()
                .find(|(_, _, _, owner)| owner.is_none_or(|owner| owner.owner == client))
        });
        let visible = hud_visible(
            session.phase,
            menu.open,
            local.map(|(_, _, alive, _)| alive.0),
        );
        let Some(mut probe) = probe else { return };
        probe.0 = visible;
        let Some((rubber, speed, _, _)) = local.filter(|_| visible) else {
            return;
        };
        probe.1 = Some(rubber_percent(rubber_fraction(rubber.0)));
        probe.2 = Some(format_speed(speed.0));
    }
}
