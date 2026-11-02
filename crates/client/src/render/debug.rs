//! Bot debug overlay: gizmos + egui panel, gated by `TRON_BOT_DEBUG`.
//!
//! Zero cost when disabled: both systems early-return before touching any
//! query beyond the resource check. The server only replicates [`BotDebug`]
//! on bot entities while the same env var is set, so with the gate off there
//! is nothing to draw and no extra bandwidth.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, Controlled, ControlledBy, Predicted};
use shared::{
    ARENA_WIDTH, BASE_SPEED, BotDebug, Direction, DisplayName, IsAlive, Player, PlayerInput,
    Position, TICK_SECS,
};

use crate::connection::{ConnectionPhase, Session};
use crate::menu::MenuState;

/// Env var gating the overlay (mirrors the server writer).
pub const BOT_DEBUG_ENV: &str = "TRON_BOT_DEBUG";
/// Sensor ray draw cap (matches the server `SENSE_HORIZON`).
pub const RAY_CAP: f32 = 240.0;
/// Duel-range ring radius (matches the server `DUEL_RANGE`).
pub const DUEL_RANGE: f32 = 480.0;
/// Foe heading extension length.
pub const FOE_HEADING_LEN: f32 = 120.0;
/// Threat ring radius.
pub const THREAT_RADIUS: f32 = 60.0;
/// Debug grid resolution per axis (matches the server snapshot grid).
pub const DEBUG_GRID_N: usize = 32;

/// Overlay gate, read once at startup.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct DebugOverlay {
    pub enabled: bool,
}

impl DebugOverlay {
    /// Read config from process environment.
    pub fn from_env() -> Self {
        Self::from_map(&crate::platform::env_var)
    }

    /// Test seam: read config from any key-value lookup.
    pub fn from_map(get: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            enabled: get(BOT_DEBUG_ENV).is_some_and(|v| is_truthy(&v)),
        }
    }
}

/// Truthy values: `1` / `true` / `yes` (case-insensitive, padded).
pub fn is_truthy(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

/// Human label for a `MODE_*` id.
pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        shared::MODE_ESCAPE => "Escape",
        shared::MODE_SURVIVAL => "Survival",
        shared::MODE_ATTACK => "Attack",
        shared::MODE_EXPLORE => "Explore",
        _ => "Unknown",
    }
}

/// Pick the alive debug bot nearest the anchor (local rider, else camera).
/// Pure so tests skip the Bevy queries.
pub fn select_debug_bot(anchor: Vec2, bots: &[(Entity, Vec2)]) -> Option<Entity> {
    bots.iter()
        .min_by(|(_, a), (_, b)| {
            anchor
                .distance_squared(*a)
                .total_cmp(&anchor.distance_squared(*b))
        })
        .map(|(entity, _)| *entity)
}

/// World position → clamped 32x32 grid cell.
pub fn cell_of(pos: Vec2) -> (usize, usize) {
    let cell = ARENA_WIDTH / DEBUG_GRID_N as f32;
    let half = ARENA_WIDTH * 0.5;
    let local = (pos + Vec2::splat(half)) / cell;
    (
        (local.x.floor().max(0.0) as usize).min(DEBUG_GRID_N - 1),
        (local.y.floor().max(0.0) as usize).min(DEBUG_GRID_N - 1),
    )
}

/// World-space centre of a debug grid cell.
pub fn cell_center(gx: usize, gy: usize) -> Vec2 {
    let cell = ARENA_WIDTH / DEBUG_GRID_N as f32;
    let half = ARENA_WIDTH * 0.5;
    Vec2::new(
        (gx as f32 + 0.5) * cell - half,
        (gy as f32 + 0.5) * cell - half,
    )
}

/// The three one-step fork positions: Hold / Left / Right.
pub fn fork_positions(pos: Vec2, dir: Vec2) -> [Vec2; 3] {
    let step = BASE_SPEED * TICK_SECS;
    [
        pos + dir * step,
        pos + shared::rotate_left(dir) * step,
        pos + shared::rotate_right(dir) * step,
    ]
}

/// The 8-ray fan directions, mirroring the server sensor.
pub fn ray_directions(dir: Vec2) -> [Vec2; 8] {
    let left = shared::rotate_left(dir);
    let right = shared::rotate_right(dir);
    [
        dir,
        left,
        right,
        // Rear ray (`-dir`): sensed/replicated/visualized only; it normally
        // sees the rider's own older trail behind.
        -dir,
        (dir + left).normalize_or_zero(),
        (dir + right).normalize_or_zero(),
        (left - dir).normalize_or_zero(),
        (right - dir).normalize_or_zero(),
    ]
}

type LocalRider = (
    With<Player>,
    With<Controlled>,
    With<Predicted>,
    With<InputMarker<PlayerInput>>,
);

type DebugBot = (With<Player>, With<BotDebug>);

/// Draw rays, enemy link, rings, forks, and cell highlights for the nearest
/// debug bot. Runs in `Update` after replication.
#[allow(clippy::type_complexity)]
pub fn draw_bot_debug(
    overlay: Res<DebugOverlay>,
    locals: Query<(&Position, Option<&ControlledBy>), LocalRider>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    bots: Query<(Entity, &Position, &Direction, &BotDebug, &IsAlive), DebugBot>,
    camera: Query<&Transform, With<Camera2d>>,
    mut gizmos: Gizmos,
) {
    if !overlay.enabled {
        return;
    }
    let anchor = debug_anchor(&locals, &clients, &camera);
    let alive: Vec<(Entity, Vec2)> = bots
        .iter()
        .filter(|(_, _, _, _, alive)| alive.0)
        .map(|(entity, pos, _, _, _)| (entity, pos.0))
        .collect();
    let Some(selected) = select_debug_bot(anchor, &alive) else {
        return;
    };
    let Ok((_, pos, dir, dbg, _)) = bots.get(selected) else {
        return;
    };
    draw_rays(&mut gizmos, pos.0, dir.0, dbg);
    draw_enemy_link(&mut gizmos, pos.0, dbg);
    draw_rings(&mut gizmos, pos.0, dbg);
    draw_forks(&mut gizmos, pos.0, dir.0, dbg);
    draw_cells(&mut gizmos, pos.0, dbg);
    draw_ray_cells(&mut gizmos, pos.0, dir.0, dbg);
}

/// Resolve the selection anchor: local rider, else camera centre, else origin.
fn debug_anchor(
    locals: &Query<(&Position, Option<&ControlledBy>), LocalRider>,
    clients: &Query<Entity, (With<Client>, With<Connected>)>,
    camera: &Query<&Transform, With<Camera2d>>,
) -> Vec2 {
    let local = clients.single().ok().and_then(|client| {
        locals
            .iter()
            .find(|(_, owner)| owner.is_none_or(|owner| owner.owner == client))
            .map(|(pos, _)| pos.0)
    });
    if let Some(pos) = local {
        return pos;
    }
    camera
        .iter()
        .next()
        .map(|t| t.translation.truncate())
        .unwrap_or(Vec2::ZERO)
}

/// Eight clearance rays, faded by how open each direction is.
fn draw_rays(gizmos: &mut Gizmos, pos: Vec2, dir: Vec2, dbg: &BotDebug) {
    let directions = ray_directions(dir);
    for (i, ray_dir) in directions.iter().enumerate() {
        let raw = dbg.rays[i];
        let len = if raw.is_finite() {
            raw.min(RAY_CAP)
        } else {
            RAY_CAP
        };
        let openness = (len / RAY_CAP).clamp(0.0, 1.0);
        let color = Color::srgba(0.2, 0.9, 0.9, 0.9 - 0.65 * openness);
        gizmos.line_2d(pos, pos + *ray_dir * len, color);
    }
}

/// Line from the bot to its foe plus the foe's heading extension.
fn draw_enemy_link(gizmos: &mut Gizmos, pos: Vec2, dbg: &BotDebug) {
    if dbg.target == Vec2::ZERO {
        return;
    }
    gizmos.line_2d(pos, dbg.target, Color::srgb(1.0, 0.9, 0.2));
    if dbg.foe_dir != Vec2::ZERO {
        gizmos.line_2d(
            dbg.target,
            dbg.target + dbg.foe_dir.normalize_or_zero() * FOE_HEADING_LEN,
            Color::srgb(1.0, 0.55, 0.15),
        );
    }
}

/// Threat ring (alpha by threat) plus the faint duel-range ring.
fn draw_rings(gizmos: &mut Gizmos, pos: Vec2, dbg: &BotDebug) {
    gizmos
        .circle_2d(
            pos,
            THREAT_RADIUS,
            Color::srgba(1.0, 0.2, 0.2, 0.15 + 0.75 * dbg.threat.clamp(0.0, 1.0)),
        )
        .resolution(32);
    gizmos
        .circle_2d(pos, DUEL_RANGE, Color::srgba(1.0, 1.0, 1.0, 0.12))
        .resolution(64);
}

/// One-step fork markers: circles for Hold/Left/Right, a red X on the worst.
fn draw_forks(gizmos: &mut Gizmos, pos: Vec2, dir: Vec2, dbg: &BotDebug) {
    let forks = fork_positions(pos, dir);
    let mut worst = 0;
    for i in 1..3 {
        if dbg.scores[i] < dbg.scores[worst] {
            worst = i;
        }
    }
    for (i, fork) in forks.iter().enumerate() {
        let color = if i == dbg.winner() {
            Color::srgb(0.3, 1.0, 0.3)
        } else {
            Color::srgb(0.6, 0.6, 0.6)
        };
        gizmos.circle_2d(*fork, 8.0, color).resolution(16);
        if i == worst && worst != dbg.winner() {
            let red = Color::srgb(1.0, 0.2, 0.2);
            gizmos.line_2d(*fork - Vec2::splat(8.0), *fork + Vec2::splat(8.0), red);
            gizmos.line_2d(
                *fork + Vec2::new(-8.0, 8.0),
                *fork + Vec2::new(8.0, -8.0),
                red,
            );
        }
    }
}

/// Bot cell (cyan) and target cell (yellow). Ray-pierced cells are drawn by
/// `draw_ray_cells`, which also needs the bot heading.
fn draw_cells(gizmos: &mut Gizmos, pos: Vec2, dbg: &BotDebug) {
    let size = ARENA_WIDTH / DEBUG_GRID_N as f32;
    gizmos.rect_2d(
        cell_center_of(cell_of(pos)),
        Vec2::splat(size),
        Color::srgba(0.2, 0.9, 0.9, 0.8),
    );
    gizmos.rect_2d(
        cell_center_of(cell_of(dbg.target)),
        Vec2::splat(size),
        Color::srgba(1.0, 0.9, 0.2, 0.8),
    );
}

/// `cell_center` under a distinct name for the highlight closures above.
fn cell_center_of(cell: (usize, usize)) -> Vec2 {
    cell_center(cell.0, cell.1)
}

/// Draw pierced cells for finite rays given the bot heading.
pub fn draw_ray_cells(gizmos: &mut Gizmos, pos: Vec2, dir: Vec2, dbg: &BotDebug) {
    let cell = ARENA_WIDTH / DEBUG_GRID_N as f32;
    let directions = ray_directions(dir);
    let mut seen = [false; DEBUG_GRID_N * DEBUG_GRID_N];
    for (i, ray_dir) in directions.iter().enumerate() {
        let raw = dbg.rays[i];
        if !raw.is_finite() {
            continue;
        }
        let len = raw.min(RAY_CAP);
        let steps = (len / (cell * 0.5)).ceil() as usize;
        for s in 0..=steps.min(64) {
            let p = pos + *ray_dir * (s as f32 * cell * 0.5).min(len);
            let (gx, gy) = cell_of(p);
            let index = gy * DEBUG_GRID_N + gx;
            if !seen[index] {
                seen[index] = true;
                gizmos.rect_2d(
                    cell_center(gx, gy),
                    Vec2::splat(cell),
                    Color::srgba(0.2, 0.9, 0.9, 0.25),
                );
            }
        }
    }
}

/// Right-side egui panel for the selected debug bot. Hidden while the menu is
/// open (same rule as the HUD); shows "no debug data" when absent.
#[allow(clippy::type_complexity)]
// Bevy injects each resource as its own parameter; the count is structural.
#[allow(clippy::too_many_arguments)]
pub fn debug_panel_ui(
    mut contexts: EguiContexts,
    overlay: Res<DebugOverlay>,
    session: Res<Session>,
    menu: Res<MenuState>,
    locals: Query<(&Position, Option<&ControlledBy>), LocalRider>,
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    bots: Query<(Entity, &Position, &BotDebug, Option<&DisplayName>, &IsAlive), DebugBot>,
    camera: Query<&Transform, With<Camera2d>>,
) {
    if !overlay.enabled || menu.open || session.phase != ConnectionPhase::Playing {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let anchor = debug_anchor(&locals, &clients, &camera);
    let alive: Vec<(Entity, Vec2)> = bots
        .iter()
        .filter(|(_, _, _, _, alive)| alive.0)
        .map(|(entity, pos, _, _, _)| (entity, pos.0))
        .collect();
    let selected = select_debug_bot(anchor, &alive).and_then(|e| bots.get(e).ok());
    egui::Area::new("bot_debug_panel".into())
        .anchor(egui::Align2::RIGHT_TOP, [-20.0, 20.0])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(180))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ui.set_max_width(220.0);
                    let Some((_, _, dbg, name, _)) = selected else {
                        ui.label(egui::RichText::new("BOT DEBUG").size(14.0).strong());
                        ui.label("no debug data");
                        return;
                    };
                    let title = name.map_or("bot", |n| n.0.as_str());
                    ui.label(
                        egui::RichText::new(format!("BOT {title}"))
                            .size(14.0)
                            .strong(),
                    );
                    ui.label(format!("mode {}", mode_name(dbg.mode)));
                    let winner = dbg.winner();
                    for (i, label) in ["H", "L", "R"].iter().enumerate() {
                        let text = format!("{label}: {:.3}", dbg.scores[i]);
                        if i == winner {
                            ui.label(
                                egui::RichText::new(text)
                                    .strong()
                                    .color(egui::Color32::GREEN),
                            );
                        } else {
                            ui.label(text);
                        }
                    }
                    ui.label(format!("threat {:.2}", dbg.threat));
                    ui.label(format!("freedom {} exits {}", dbg.freedom, dbg.exits));
                    ui.label(format!("turn_prob {:.2}", dbg.turn_prob));
                    ui.label(format!(
                        "agg {:.2} cau {:.2} grd {:.2}",
                        dbg.aggression, dbg.caution, dbg.greed
                    ));
                    ui.label(format!("la {} rx {}", dbg.lookahead, dbg.reaction));
                    ui.label(format!(
                        "head-on {} converging {}",
                        dbg.head_on(),
                        dbg.converging()
                    ));
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn overlay(pairs: &[(&str, &str)]) -> DebugOverlay {
        let m = map(pairs);
        DebugOverlay::from_map(&|k| m.get(k).cloned())
    }

    #[test]
    fn overlay_off_by_default() {
        assert!(!overlay(&[]).enabled);
        assert!(!overlay(&[(BOT_DEBUG_ENV, "0")]).enabled);
        assert!(!overlay(&[(BOT_DEBUG_ENV, "false")]).enabled);
    }

    #[test]
    fn overlay_truthy_values_enable() {
        for raw in ["1", "true", "TRUE", "yes", " Yes "] {
            assert!(overlay(&[(BOT_DEBUG_ENV, raw)]).enabled, "{raw}");
        }
    }

    #[test]
    fn nearest_bot_wins_selection() {
        let a = Entity::from_bits(1);
        let b = Entity::from_bits(2);
        let bots = vec![(a, Vec2::new(100.0, 0.0)), (b, Vec2::new(10.0, 0.0))];
        assert_eq!(select_debug_bot(Vec2::ZERO, &bots), Some(b));
        assert_eq!(select_debug_bot(Vec2::ZERO, &[]), None);
    }

    #[test]
    fn mode_names_cover_all_modes() {
        assert_eq!(mode_name(shared::MODE_ESCAPE), "Escape");
        assert_eq!(mode_name(shared::MODE_SURVIVAL), "Survival");
        assert_eq!(mode_name(shared::MODE_ATTACK), "Attack");
        assert_eq!(mode_name(shared::MODE_EXPLORE), "Explore");
        assert_eq!(mode_name(99), "Unknown");
    }

    #[test]
    fn grid_cells_clamp_to_arena() {
        assert_eq!(cell_of(Vec2::new(-5000.0, -5000.0)), (0, 0));
        assert_eq!(
            cell_of(Vec2::new(5000.0, 5000.0)),
            (DEBUG_GRID_N - 1, DEBUG_GRID_N - 1)
        );
        let (gx, gy) = cell_of(Vec2::ZERO);
        assert!(cell_center(gx, gy).distance(Vec2::ZERO) < ARENA_WIDTH / DEBUG_GRID_N as f32);
    }

    #[test]
    fn forks_step_off_heading() {
        let forks = fork_positions(Vec2::ZERO, Vec2::X);
        let step = BASE_SPEED * TICK_SECS;
        assert_eq!(forks[0], Vec2::new(step, 0.0));
        assert_eq!(forks[1], Vec2::new(0.0, step));
        assert_eq!(forks[2], Vec2::new(0.0, -step));
    }

    #[test]
    fn ray_directions_mirror_eight_ray_server_fan() {
        let dirs = ray_directions(Vec2::X);
        assert_eq!(dirs.len(), 8);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let expected = [
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(0.0, -1.0),
            Vec2::new(-1.0, 0.0),
            Vec2::new(s, s),
            Vec2::new(s, -s),
            Vec2::new(-s, s),
            Vec2::new(-s, -s),
        ];
        for (got, want) in dirs.iter().zip(expected.iter()) {
            assert!(
                got.distance(*want) < 1e-5,
                "fan order mismatch: {dirs:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn ray_cells_light_behind_for_rear_ray() {
        // A finite rear ray (index 3) must light cells behind the rider.
        let pos = Vec2::ZERO;
        let dir = Vec2::X;
        let mut dbg = BotDebug::default();
        dbg.rays[3] = 100.0;
        let behind = pos + ray_directions(dir)[3] * dbg.rays[3];
        assert!(behind.x < 0.0, "rear ray must point behind: {behind:?}");
        let (gx, _) = cell_of(behind);
        let (home_gx, _) = cell_of(pos);
        assert!(gx < home_gx, "behind cell must differ");
    }
}
