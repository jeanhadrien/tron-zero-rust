//! `TRON_BOT_DEBUG` gate + [`shared::BotDebug`] snapshot builder.
//!
//! Read once at startup into [`BotDebugConfig`]. When disabled (the default)
//! the writer never inserts the component, so production pays zero bandwidth
//! and emits no per-decision logs.

use bevy::prelude::*;
use shared::{BotDebug, FLAG_CONVERGING, FLAG_HEAD_ON};

use bots::config::BotKnobs;
use bots::perceive::Perception;
use bots::strategy::{Action, Strategy, converging, head_on, nearest_enemy};
use bots::world::WorldSnapshot;

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

/// Build the replicated snapshot from a finished decision.
///
/// `scores` is the same array `decide` argmaxed over — the caller computes it
/// via `strategy.score(...)` so `decide` itself stays untouched.
#[allow(clippy::too_many_arguments)]
pub fn build_debug(
    strategy: &dyn Strategy,
    perception: &Perception,
    scores: [f32; 3],
    world: &WorldSnapshot,
    action: Action,
    knobs: &BotKnobs,
) -> BotDebug {
    let me = world.riders.get(perception.rider);
    let foe = nearest_enemy(world, perception.rider).map(|(_, rider)| rider);
    let (target, foe_dir) = foe.map_or((Vec2::ZERO, Vec2::ZERO), |f| (f.pos, f.dir));
    let mut flags = 0u8;
    if let (Some(me), Some(foe)) = (me, foe)
        && foe.pos != Vec2::ZERO
        && head_on(me.pos, me.dir, foe.pos, foe.dir)
    {
        flags |= FLAG_HEAD_ON;
    }
    if let (Some(me), Some(foe)) = (me, foe)
        && converging(me.pos, me.dir, foe.pos, foe.dir, knobs)
    {
        flags |= FLAG_CONVERGING;
    }
    BotDebug {
        mode: strategy.mode(perception).to_u8(),
        action: action.index() as u8,
        scores,
        threat: perception.threat,
        freedom: perception.freedom,
        exits: perception.exits,
        turn_prob: perception.opponent_turn_prob,
        aggression: knobs.aggression,
        caution: knobs.caution,
        greed: knobs.greed,
        lookahead: knobs.lookahead_ticks,
        reaction: knobs.reaction_ticks,
        target,
        foe_dir,
        rays: perception.rays,
        flags,
    }
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
    use bots::config::{Difficulty, Personality};
    use bots::forecast::PhysicsForecaster;
    use bots::perceive::RAY_COUNT;
    use bots::strategy::strategy_for;
    use std::collections::HashMap;
    use std::sync::Arc;

    use bots::world::RiderSnapshot;

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
        let knobs = Difficulty(2000).knobs(Personality::Duelist);
        let perception = Perception {
            rays: [120.0; RAY_COUNT],
            freedom: 400,
            exits: 3,
            threat: 0.5,
            memory_trend: 0.0,
            opponent_turn_prob: 0.25,
            tick: 9,
            rider: 0,
        };
        let strategy = strategy_for(Personality::Duelist);
        let forecaster = PhysicsForecaster;
        let scores = strategy.score(&perception, &world, &forecaster, &knobs);
        let dbg = build_debug(strategy, &perception, scores, &world, Action::Hold, &knobs);
        assert_eq!(dbg.scores, scores);
        assert_eq!(dbg.action, Action::Hold.index() as u8);
        assert_eq!(dbg.threat, 0.5);
        assert_eq!(dbg.freedom, 400);
        assert_eq!(dbg.exits, 3);
        assert_eq!(dbg.turn_prob, 0.25);
        assert_eq!(dbg.target, Vec2::new(100.0, 0.0));
        assert_eq!(dbg.foe_dir, Vec2::NEG_X);
        assert_eq!(dbg.rays, [120.0; RAY_COUNT]);
        assert_eq!(dbg.aggression, knobs.aggression);
        assert_eq!(dbg.lookahead, knobs.lookahead_ticks);
        assert_eq!(dbg.reaction, knobs.reaction_ticks);
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
            freedom: 777,
            ..Default::default()
        };
        world.insert_resource(Fixture {
            cfg: cfg(&[(BOT_DEBUG_ENV, "1")]),
            entity,
            dbg,
        });
        world.run_system_once(fixture_system).unwrap();
        assert_eq!(world.get::<BotDebug>(entity).unwrap().freedom, 777);
    }
}
