//! Frameless rubber meter (bottom-left) and ping badge (top-right), drawn
//! with egui.
//!
//! Values come from the local predicted rider; the meter shows only while
//! Playing, menu closed, local rider alive.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, Controlled, ControlledBy, Link, Predicted};
use shared::{BASE_RUBBER, IsAlive, Player, PlayerInput, Rubber};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;
use crate::sim::NetSim;
use crate::theme::Theme;

// Fixed bar width. Without the max, the Area auto-sizes to content and
// `available_width()` below is unbounded, which stretched the bar across the
// screen.
const BAR_WIDTH: f32 = 120.0;
// HUD elements are drawn translucent so they stay out of the way.
const HUD_ALPHA: f32 = 0.6;

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

// Measured round-trip time as "ping 21ms"; empty until Lightyear has a sample.
pub fn format_ping(rtt: std::time::Duration) -> Option<String> {
    (!rtt.is_zero()).then(|| format!("ping {}ms", rtt.as_millis()))
}

/// Top-right badge while playing: measured ping and (loudly) whether the
/// network is being simulated, so it is never mistaken for real conditions.
/// Unlike the meter it stays up while dead.
pub fn status_badge_ui(
    mut contexts: EguiContexts,
    session: Res<Session>,
    menu: Res<MenuState>,
    theme: Res<Theme>,
    sim: Option<Res<NetSim>>,
    links: Query<&Link, With<Client>>,
) {
    if session.phase != ConnectionPhase::Playing || menu.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let ping = links
        .iter()
        .next()
        .and_then(|link| format_ping(link.stats.rtt));
    egui::Area::new("hud_badge".into())
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            if let Some(ping) = ping {
                ui.label(
                    egui::RichText::new(ping)
                        .size(12.0)
                        .color(theme.body.egui().gamma_multiply(HUD_ALPHA)),
                );
            }
            if let Some(sim) = sim {
                ui.label(
                    egui::RichText::new(sim.label())
                        .size(12.0)
                        .strong()
                        .color(theme.warn.egui()),
                );
            }
        });
}

#[allow(clippy::type_complexity)]
pub fn hud_ui(
    mut contexts: EguiContexts,
    session: Res<Session>,
    menu: Res<MenuState>,
    theme: Res<Theme>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    players: Query<
        (&Rubber, &IsAlive, Option<&ControlledBy>),
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
            .find(|(_, _, owner)| owner.is_none_or(|owner| owner.owner == client))
    });
    if !hud_visible(session.phase, menu.open, local.map(|(_, alive, _)| alive.0)) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Some((rubber, _, _)) = local else {
        return;
    };
    let fraction = rubber_fraction(rubber.0);
    let color = theme.rubber_color(fraction);

    // A small frameless meter pinned to the bottom-left corner;
    // non-interactive so gameplay clicks pass through. Painted in the default
    // Middle order — the menu hides the HUD outright when open, so it always
    // wins any tie.
    egui::Area::new("hud_rubber".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0, -16.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_width(BAR_WIDTH);
            ui.label(
                egui::RichText::new(format!("{}%", rubber_percent(fraction)))
                    .size(16.0)
                    .color(color.gamma_multiply(HUD_ALPHA)),
            );
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 0.0, theme.track.egui().gamma_multiply(0.5));
            let fill = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(rect.width() * fraction, rect.height()),
            );
            ui.painter()
                .rect_filled(fill, 0.0, color.gamma_multiply(HUD_ALPHA));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::{PeerId, RemoteId};

    #[test]
    fn ping_text_waits_for_a_sample() {
        assert_eq!(format_ping(std::time::Duration::ZERO), None);
        assert_eq!(
            format_ping(std::time::Duration::from_millis(21)).as_deref(),
            Some("ping 21ms")
        );
    }

    #[test]
    fn rubber_math_matches_old_meters() {
        assert_eq!(rubber_fraction(BASE_RUBBER / 2.0), 0.5);
        assert_eq!(rubber_percent(0.5), 50);
        assert_eq!(rubber_percent(1.0), 100);
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
        world.init_resource::<Probe>();

        // No client yet: nothing visible.
        world.run_system_once(update_probe).unwrap();
        assert!(!world.resource::<Probe>().0);

        world.spawn((Client::default(), RemoteId(PeerId::Server), Connected));
        // Confirmed/remote copies must never supply the HUD values.
        world.spawn((Player, Rubber(1.0), IsAlive(true)));
        let rider = world
            .spawn((
                Player,
                Controlled,
                Predicted,
                InputMarker::<PlayerInput>::default(),
                Rubber(BASE_RUBBER / 2.0),
                IsAlive(true),
            ))
            .id();
        world.resource_mut::<Session>().phase = ConnectionPhase::Playing;
        world.resource_mut::<MenuState>().open = false;
        world.run_system_once(update_probe).unwrap();
        let probe = world.resource::<Probe>();
        assert!(probe.0);
        assert_eq!(probe.1, Some(50));

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
            .insert((IsAlive(true), Rubber(BASE_RUBBER)));
        world.run_system_once(update_probe).unwrap();
        let probe = world.resource::<Probe>();
        assert!(probe.0);
        assert_eq!(probe.1, Some(100));

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
    struct Probe(bool, Option<u32>);

    #[allow(clippy::type_complexity)]
    fn update_probe(
        session: Res<Session>,
        menu: Res<MenuState>,
        clients: Query<Entity, (With<Client>, With<Connected>)>,
        players: Query<
            (&Rubber, &IsAlive, Option<&ControlledBy>),
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
                .find(|(_, _, owner)| owner.is_none_or(|owner| owner.owner == client))
        });
        let visible = hud_visible(session.phase, menu.open, local.map(|(_, alive, _)| alive.0));
        let Some(mut probe) = probe else { return };
        probe.0 = visible;
        let Some((rubber, _, _)) = local.filter(|_| visible) else {
            return;
        };
        probe.1 = Some(rubber_percent(rubber_fraction(rubber.0)));
    }
}
