//! `TRON_BOT_DEBUG` gate + [`shared::BotDebug`] snapshot builder.
//!
//! Read once at startup into [`BotDebugConfig`]. When disabled (the default)
//! the writer never inserts the component, so production pays zero bandwidth
//! and emits no per-decision logs.

use bevy::prelude::*;
use shared::physics::nearest;
use shared::{BotDebug, FLAG_CONVERGING, FLAG_HEAD_ON, rotate_left, rotate_right};

use bots::config::BotKnobs;
use bots::plan::Action;
use bots::planner::Report;
use bots::world::{RiderSnapshot, WorldSnapshot};

/// Env var gating the bot debug overlay, server writer included.
pub const BOT_DEBUG_ENV: &str = "TRON_BOT_DEBUG";

/// Server-side gate, read once at startup.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct BotDebugConfig {
    pub enabled: bool,
}

impl BotDebugConfig {
    /// Read config from process environment.
    pub fn from_env() -> Self {
        Self::from_map(&|key| std::env::var(key).ok())
    }

    /// Test seam: read config from any key-value lookup.
    pub fn from_map(get: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            enabled: get(BOT_DEBUG_ENV).is_some_and(|v| is_truthy(&v)),
        }
    }
}

/// Truthy values: `1` / `true` / `yes` (case-insensitive, padded). Everything
/// else — including unset — is off.
pub fn is_truthy(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

/// Direct env probe for entity spawn paths that run without resource access.
pub fn enabled_from_env() -> bool {
    std::env::var(BOT_DEBUG_ENV)
        .map(|v| is_truthy(&v))
        .unwrap_or(false)
}

/// Distance at which an enemy stops counting as a threat.
const THREAT_RANGE: f32 = 600.0;
/// Range of the "converging" flag.
const DUEL_RANGE: f32 = 480.0;

/// Build the replicated snapshot from a finished planning round: the report
/// the planner returned for rider `index` in `world`, and the action it
/// emitted on this tick.
pub fn build_debug(
    report: &Report,
    world: &WorldSnapshot,
    index: usize,
    action: Action,
    knobs: &BotKnobs,
) -> BotDebug {
    let me = &world.riders[index];
    let foe = nearest_foe(world, index);
    let (target, foe_dir) = foe.map_or((Vec2::ZERO, Vec2::ZERO), |f| (f.pos, f.dir));
    let mut flags = 0u8;
    if let Some(foe) = foe {
        if head_on(me.pos, me.dir, foe.pos, foe.dir) {
            flags |= FLAG_HEAD_ON;
        }
        if converging(me.pos, me.dir, foe.pos, foe.dir) {
            flags |= FLAG_CONVERGING;
        }
    }
    BotDebug {
        mode: report.mode.to_u8(),
        action: action.index() as u8,
        scores: report.scores,
        threat: foe.map_or(0.0, |f| {
            (1.0 - me.pos.distance(f.pos) / THREAT_RANGE).clamp(0.0, 1.0)
        }),
        area: report.area,
        open: report.open.min(u32::from(u8::MAX)) as u8,
        noise: knobs.noise,
        space: report.terms.space,
        reach: report.terms.reach,
        clear: report.terms.clear,
        horizon: report.horizon,
        reaction: knobs.reaction_ticks,
        target,
        foe_dir,
        rays: rays(world, index),
        flags,
    }
}

/// Distance to the nearest obstacle along eight directions around the rider:
/// ahead, left, right, behind, then the four diagonals. Infinite = none.
fn rays(world: &WorldSnapshot, index: usize) -> [f32; 8] {
    let me = &world.riders[index];
    let (left, right) = (rotate_left(me.dir), rotate_right(me.dir));
    let directions = [
        me.dir,
        left,
        right,
        -me.dir,
        (me.dir + left).normalize_or_zero(),
        (me.dir + right).normalize_or_zero(),
        (left - me.dir).normalize_or_zero(),
        (right - me.dir).normalize_or_zero(),
    ];
    let mut out = [f32::INFINITY; 8];
    for (i, direction) in directions.iter().enumerate() {
        // As in the sim: only the front sensor uses forward semantics.
        let side = i != 0;
        out[i] = nearest(
            world.obstacles(),
            Some(index as u64),
            me.pos,
            *direction,
            side,
        );
    }
    out
}

fn nearest_foe(world: &WorldSnapshot, index: usize) -> Option<&RiderSnapshot> {
    let me = world.riders.get(index)?;
    world
        .riders
        .iter()
        .enumerate()
        .filter(|(i, other)| *i != index && other.alive)
        .map(|(_, other)| other)
        .min_by(|a, b| me.pos.distance(a.pos).total_cmp(&me.pos.distance(b.pos)))
}

/// In range, and at least one of the two faces the other.
fn converging(me_pos: Vec2, me_dir: Vec2, foe_pos: Vec2, foe_dir: Vec2) -> bool {
    let to_foe = foe_pos - me_pos;
    let dist = to_foe.length();
    if dist < 1.0 {
        return true;
    }
    if dist > DUEL_RANGE {
        return false;
    }
    let axis = to_foe / dist;
    me_dir.dot(axis) > 0.0 || foe_dir.dot(-axis) > 0.0
}

/// Both riders committed at each other's noses.
fn head_on(me_pos: Vec2, me_dir: Vec2, foe_pos: Vec2, foe_dir: Vec2) -> bool {
    let to_foe = foe_pos - me_pos;
    let dist = to_foe.length();
    if dist < 1.0 {
        return true;
    }
    let axis = to_foe / dist;
    me_dir.dot(foe_dir) < -0.5 && me_dir.dot(axis) > 0.5 && foe_dir.dot(-axis) > 0.5
}

/// Insert the snapshot when the gate is on; a no-op otherwise (keeps the
/// writer's disabled path to a single branch).
pub fn insert_if_enabled(
    commands: &mut Commands,
    cfg: &BotDebugConfig,
    entity: Entity,
    dbg: BotDebug,
) {
    if cfg.enabled {
        commands.entity(entity).insert(dbg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bots::config::{BotBuild, Difficulty, Personality};
    use bots::planner::{Params, plan};
    use bots::schedule::Tier;
    use bots::space::Scratch;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn cfg(pairs: &[(&str, &str)]) -> BotDebugConfig {
        let m = map(pairs);
        BotDebugConfig::from_map(&|k| m.get(k).cloned())
    }

    #[test]
    fn debug_off_by_default() {
        assert!(!cfg(&[]).enabled);
        assert!(!cfg(&[(BOT_DEBUG_ENV, "0")]).enabled);
        assert!(!cfg(&[(BOT_DEBUG_ENV, "")]).enabled);
        assert!(!cfg(&[(BOT_DEBUG_ENV, "false")]).enabled);
        assert!(!cfg(&[(BOT_DEBUG_ENV, "no")]).enabled);
    }

    #[test]
    fn debug_truthy_values_enable() {
        for raw in ["1", "true", "TRUE", "yes", " Yes "] {
            assert!(cfg(&[(BOT_DEBUG_ENV, raw)]).enabled, "{raw}");
        }
    }

    fn rider(id: u64, pos: Vec2, dir: Vec2) -> RiderSnapshot {
        RiderSnapshot {
            id,
            pos,
            dir,
            trail: shared::Trail(vec![pos - dir * 10.0]),
            alive: true,
            rubber: 0.0,
            speed_mult: 1.0,
            target_speed_mult: 1.0,
        }
    }

    fn head_on_world() -> WorldSnapshot {
        WorldSnapshot::new(
            Arc::new(Vec::new()),
            vec![
                rider(1, Vec2::new(-100.0, 0.0), Vec2::X),
                rider(2, Vec2::new(100.0, 0.0), Vec2::NEG_X),
            ],
            9,
        )
    }

    #[test]
    fn build_debug_values_are_sane() {
        let world = head_on_world();
        let tuning = bots::roster::builtin_tuning().unwrap();
        let build = BotBuild::from_difficulty("t", Personality::Duelist, Difficulty(2000), tuning);
        let params = Params::new(&build, Tier::Full, 1);
        let outcome = plan(&world, 0, &params, &mut Scratch::default(), None, None);
        let action = outcome.maneuver.first_action();
        let dbg = build_debug(&outcome.report, &world, 0, action, &build.knobs);
        assert_eq!(dbg.scores, outcome.report.scores);
        assert_eq!(dbg.action, action.index() as u8);
        assert_eq!(dbg.area, outcome.report.area);
        assert!((dbg.threat - 0.666).abs() < 0.01, "{}", dbg.threat);
        assert_eq!(dbg.target, Vec2::new(100.0, 0.0));
        assert_eq!(dbg.foe_dir, Vec2::NEG_X);
        assert!(dbg.rays[0].is_finite());
        assert_eq!(dbg.horizon, outcome.report.horizon);
        assert_eq!(dbg.reaction, build.knobs.reaction_ticks);
        assert_ne!(dbg.flags & FLAG_HEAD_ON, 0);
        assert_ne!(dbg.flags & FLAG_CONVERGING, 0);
        assert!(dbg.scores.iter().all(|s| s.is_finite()));
    }

    // Fixture-driven one-shot systems: `run_system_once` applies the command
    // buffer, so these exercise the real insert/skip path.
    #[derive(Resource)]
    struct Fixture {
        cfg: BotDebugConfig,
        entity: Entity,
        dbg: BotDebug,
    }

    fn fixture_system(mut commands: Commands, fixture: Res<Fixture>) {
        insert_if_enabled(&mut commands, &fixture.cfg, fixture.entity, fixture.dbg);
    }

    #[test]
    fn writer_off_inserts_no_component() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        world.insert_resource(Fixture {
            cfg: cfg(&[]),
            entity,
            dbg: BotDebug::default(),
        });
        world.run_system_once(fixture_system).unwrap();
        assert!(world.get::<BotDebug>(entity).is_none());
    }

    #[test]
    fn writer_on_inserts_snapshot() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let dbg = BotDebug {
            area: 777,
            ..Default::default()
        };
        world.insert_resource(Fixture {
            cfg: cfg(&[(BOT_DEBUG_ENV, "1")]),
            entity,
            dbg,
        });
        world.run_system_once(fixture_system).unwrap();
        assert_eq!(world.get::<BotDebug>(entity).unwrap().area, 777);
    }
}
