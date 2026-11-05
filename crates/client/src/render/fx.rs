//! Render-only particles and one-shot effects, drawn as gizmos.
//!
//! Nothing here touches the simulation: effects are triggered by watching what
//! riders look like (alive or not, sliding) and are never replicated.
//!
//! * **Derez**: a rider dies, its color bursts into streaks and a ring.
//! * **Rezz**: a rider appears, a ring collapses onto it.
//! * **Sparks**: a rider grinding (`IsSliding`: a side sensor within reach of a
//!   wall or another rider's trail) sheds sparks.
//!
//! Effects can be switched off ([`FxSettings`]); the theme scales how many
//! particles each effect makes.

use bevy::color::Mix;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use shared::{Direction, IsAlive, IsSliding, Player, PlayerColor, SpeedMult};

use super::player::RIDER_RADIUS;
use super::smooth::{RenderSet, Rendered};
use crate::theme::Theme;

/// Hard cap so a pile-up of deaths can never flood the frame.
const MAX_PARTICLES: usize = 600;
/// Longest frame step effects advance by (a hidden tab resumes with a huge one).
const MAX_STEP_SECS: f32 = 0.1;
/// Fraction of velocity kept per second.
const DRAG_PER_SEC: f32 = 0.05;

/// Sparks per second while grinding at base speed; scales with the speed
/// multiplier, so a boosted rider throws more.
const SPARKS_PER_SEC: f32 = 30.0;

/// Streaks in a derez burst, before the theme's scale.
const DEREZ_STREAKS: f32 = 22.0;
/// A rider first seen with less trail than this has just spawned (as opposed
/// to one already racing when we connected): only those get a rezz ring.
const FRESH_SPAWN_TRAIL: f32 = 100.0;
const RING_RADIUS_WIDE: f32 = 80.0;

/// Player toggle for all effects (reduced-motion option).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxSettings {
    pub enabled: bool,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    /// A short line trailing behind its motion.
    Streak { len: f32 },
    /// A circle whose radius moves from `from` to `to` over its life.
    Ring { from: f32, to: f32 },
}

#[derive(Debug, Clone, Copy)]
struct Particle {
    pos: Vec2,
    vel: Vec2,
    age: f32,
    life: f32,
    color: Color,
    shape: Shape,
}

impl Particle {
    /// 0 at birth, 1 at death.
    fn progress(&self) -> f32 {
        (self.age / self.life).clamp(0.0, 1.0)
    }
}

/// Live particles plus the little state needed to spot events.
#[derive(Resource)]
pub struct Fx {
    particles: Vec<Particle>,
    /// Last seen `IsAlive` per rider, to catch deaths and spawns.
    seen: HashMap<Entity, bool>,
    rng: u32,
}

impl Default for Fx {
    fn default() -> Self {
        Self {
            particles: Vec::new(),
            seen: HashMap::default(),
            rng: 0x9E37_79B9,
        }
    }
}

impl Fx {
    /// xorshift32 in [0, 1): visuals only, no need for a real RNG (or wasm entropy).
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.random()
    }

    fn push(&mut self, particle: Particle) {
        if self.particles.len() < MAX_PARTICLES {
            self.particles.push(particle);
        }
    }

    /// The rider's color bursts outward.
    fn derez(&mut self, at: Vec2, color: Color, scale: f32) {
        for _ in 0..(DEREZ_STREAKS * scale).round() as u32 {
            let angle = self.range(0.0, std::f32::consts::TAU);
            let speed = self.range(120.0, 320.0);
            let len = self.range(6.0, 14.0);
            let life = self.range(0.4, 0.8);
            self.push(Particle {
                pos: at,
                vel: Vec2::from_angle(angle) * speed,
                age: 0.0,
                life,
                color,
                shape: Shape::Streak { len },
            });
        }
        self.push(Particle {
            pos: at,
            vel: Vec2::ZERO,
            age: 0.0,
            life: 0.45,
            color,
            shape: Shape::Ring {
                from: RIDER_RADIUS,
                to: RING_RADIUS_WIDE,
            },
        });
    }

    /// A ring collapses onto the new rider.
    fn rezz(&mut self, at: Vec2, color: Color) {
        self.push(Particle {
            pos: at,
            vel: Vec2::ZERO,
            age: 0.0,
            life: 0.35,
            color,
            shape: Shape::Ring {
                from: RING_RADIUS_WIDE,
                to: RIDER_RADIUS,
            },
        });
    }

    /// One spark thrown backwards from a grinding rider.
    fn spark(&mut self, at: Vec2, heading: Vec2, color: Color) {
        let side = Vec2::new(-heading.y, heading.x);
        let back = self.range(60.0, 160.0);
        let sideways = self.range(-120.0, 120.0);
        let len = self.range(3.0, 7.0);
        let life = self.range(0.15, 0.35);
        self.push(Particle {
            pos: at,
            vel: -heading * back + side * sideways,
            age: 0.0,
            life,
            color: color.mix(&Color::WHITE, 0.5),
            shape: Shape::Streak { len },
        });
    }
}

/// Advance every particle by `dt` seconds and drop the expired ones.
fn step_particles(particles: &mut Vec<Particle>, dt: f32) {
    let drag = DRAG_PER_SEC.powf(dt);
    for particle in particles.iter_mut() {
        particle.age += dt;
        particle.pos += particle.vel * dt;
        particle.vel *= drag;
    }
    particles.retain(|particle| particle.age < particle.life);
}

fn trail_length(points: &[Vec2]) -> f32 {
    points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}

/// Spot deaths, spawns and grinding, and make the matching particles.
#[allow(clippy::type_complexity)]
fn emit_fx(
    time: Res<Time>,
    settings: Res<FxSettings>,
    theme: Res<Theme>,
    riders: Query<
        (
            Entity,
            &Rendered,
            &Direction,
            &PlayerColor,
            &IsAlive,
            &IsSliding,
            &SpeedMult,
        ),
        With<Player>,
    >,
    mut fx: ResMut<Fx>,
) {
    let dt = time.delta_secs().min(MAX_STEP_SECS);
    for (entity, rendered, dir, color, alive, sliding, speed) in &riders {
        let was_alive = fx.seen.insert(entity, alive.0);
        if !settings.enabled {
            continue;
        }
        let color = Color::srgb_u32(color.0);
        match (was_alive, alive.0) {
            (Some(true), false) => fx.derez(rendered.head, color, theme.fx_scale),
            (Some(false), true) => fx.rezz(rendered.head, color),
            (None, true) if trail_length(&rendered.trail) < FRESH_SPAWN_TRAIL => {
                fx.rezz(rendered.head, color)
            }
            _ => {}
        }
        if alive.0 && sliding.0 {
            let expected = SPARKS_PER_SEC * speed.0 * dt * theme.fx_scale;
            let mut count = expected.floor() as u32;
            if fx.random() < expected.fract() {
                count += 1;
            }
            let heading = rendered.heading(dir.0);
            for _ in 0..count {
                fx.spark(rendered.head, heading, color);
            }
        }
    }
    fx.seen.retain(|entity, _| riders.contains(*entity));
}

fn update_fx(time: Res<Time>, mut fx: ResMut<Fx>) {
    step_particles(&mut fx.particles, time.delta_secs().min(MAX_STEP_SECS));
}

fn draw_fx(fx: Res<Fx>, mut gizmos: Gizmos) {
    for particle in &fx.particles {
        let t = particle.progress();
        // Quadratic fade: bright at first, gone before the end.
        let color = particle.color.with_alpha((1.0 - t) * (1.0 - t));
        match particle.shape {
            Shape::Streak { len } => {
                let tail = -particle.vel.normalize_or_zero() * len;
                gizmos.line_2d(particle.pos, particle.pos + tail, color);
            }
            Shape::Ring { from, to } => {
                gizmos
                    .circle_2d(particle.pos, from + (to - from) * t, color)
                    .resolution(32);
            }
        }
    }
}

/// Register the effect resources and systems. Runs with the other draw
/// systems, after the render poses are up to date.
pub fn add_fx(app: &mut App) {
    app.init_resource::<Fx>();
    app.init_resource::<FxSettings>();
    app.add_systems(
        Update,
        (emit_fx, update_fx, draw_fx)
            .chain()
            .in_set(RenderSet::Draw),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn streak(life: f32) -> Particle {
        Particle {
            pos: Vec2::ZERO,
            vel: Vec2::X * 100.0,
            age: 0.0,
            life,
            color: Color::WHITE,
            shape: Shape::Streak { len: 5.0 },
        }
    }

    #[test]
    fn particles_move_slow_down_and_expire() {
        let mut particles = vec![streak(0.5), streak(0.1)];
        step_particles(&mut particles, 0.2);
        assert_eq!(particles.len(), 1, "the short-lived one expired");
        assert!(particles[0].pos.x > 0.0);
        assert!(particles[0].vel.x < 100.0);
    }

    #[test]
    fn derez_makes_streaks_and_a_ring_within_the_cap() {
        let mut fx = Fx::default();
        fx.derez(Vec2::ZERO, Color::WHITE, 1.0);
        assert_eq!(fx.particles.len(), DEREZ_STREAKS as usize + 1);
        for _ in 0..100 {
            fx.derez(Vec2::ZERO, Color::WHITE, 2.0);
        }
        assert_eq!(fx.particles.len(), MAX_PARTICLES);
    }

    fn rider(trail: Vec<Vec2>, alive: bool) -> impl Bundle {
        (
            Player,
            Rendered {
                head: Vec2::ZERO,
                trail,
            },
            Direction(Vec2::X),
            PlayerColor(0xff0000),
            IsAlive(alive),
            IsSliding(false),
            SpeedMult(1.0),
        )
    }

    fn fx_world() -> World {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Fx>();
        world.init_resource::<FxSettings>();
        world.init_resource::<Theme>();
        world
    }

    fn particles(world: &World) -> usize {
        world.resource::<Fx>().particles.len()
    }

    fn long_trail() -> Vec<Vec2> {
        vec![Vec2::new(-500.0, 0.0), Vec2::ZERO]
    }

    #[test]
    fn a_death_bursts_once_and_a_known_respawn_rezzes() {
        let mut world = fx_world();
        let entity = world.spawn(rider(long_trail(), true)).id();
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), 0, "already racing when first seen");

        world.entity_mut(entity).insert(IsAlive(false));
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), DEREZ_STREAKS as usize + 1);
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), DEREZ_STREAKS as usize + 1, "only once");

        world.entity_mut(entity).insert(IsAlive(true));
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), DEREZ_STREAKS as usize + 2, "rezz ring");
    }

    #[test]
    fn only_a_fresh_spawn_rezzes_on_first_sight() {
        let mut world = fx_world();
        world.spawn(rider(vec![Vec2::ZERO], true));
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), 1);
    }

    #[test]
    fn disabled_effects_stay_silent_and_do_not_burst_on_re_enable() {
        let mut world = fx_world();
        world.resource_mut::<FxSettings>().enabled = false;
        let entity = world.spawn(rider(long_trail(), true)).id();
        world.run_system_once(emit_fx).unwrap();
        world.entity_mut(entity).insert(IsAlive(false));
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), 0);
        world.resource_mut::<FxSettings>().enabled = true;
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(particles(&world), 0, "the death was already seen");
    }

    #[test]
    fn despawned_riders_are_forgotten() {
        let mut world = fx_world();
        let entity = world.spawn(rider(long_trail(), true)).id();
        world.run_system_once(emit_fx).unwrap();
        assert_eq!(world.resource::<Fx>().seen.len(), 1);
        world.despawn(entity);
        world.run_system_once(emit_fx).unwrap();
        assert!(world.resource::<Fx>().seen.is_empty());
    }

    #[test]
    fn random_stays_in_unit_range() {
        let mut fx = Fx::default();
        for _ in 0..1000 {
            let value = fx.random();
            assert!((0.0..1.0).contains(&value));
        }
    }
}
