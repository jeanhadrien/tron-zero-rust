//! Render state, kept apart from simulation state.
//!
//! The sim ticks at 120 Hz in `FixedUpdate`, frames land anywhere between
//! ticks. Nothing here touches sim components: each rider gets a [`Rendered`]
//! pose (head + trail polyline) that the draw systems and the camera read.
//!
//! * Own rider: the head is interpolated between the last two ticks (one tick
//!   of visual delay, no extrapolation). A rollback correction does not jump:
//!   the drawn head lags behind the corrected sim head along the trail (by
//!   exactly the correction: predicted vs corrected head at the same tick) and
//!   catches up within about 100 ms. Anything that is not a plain correction
//!   (respawn, death, first sight) snaps; a timeline resync only drops the lag.
//! * Opponents: the replicated `Trail` is already interpolated by Lightyear
//!   (`shared::polyline::interpolate_trail`); the body is drawn from its head.

use bevy::prelude::*;
use lightyear::prelude::client::InputTimelineConfig;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{InterpolationSystems, Predicted, RollbackSystems, SyncEvent};
use shared::polyline::{arc_to_end, truncate_back};
use shared::{IsAlive, LifeGeneration, Player, PlayerInput, Position, Trail};

/// Time constant of the rollback-correction catch-up (4x it is about 100 ms).
const LAG_DECAY_SECS: f32 = 0.025;
/// Below this the remaining lag is dropped.
const LAG_EPSILON: f32 = 0.05;
/// Safety net: never trail the sim head by more than a few ticks of travel.
const MAX_LAG: f32 = 40.0;
/// How far from a trail a predicted head may be and still count as "on" it.
const ON_TRAIL_TOLERANCE: f32 = 2.0;
/// A respawn farther than this from the old head is a teleport (camera eases).
const TELEPORT_DISTANCE: f32 = 50.0;

/// Ordering: render state is built after the fixed loop, before anything draws.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenderSet {
    State,
    Draw,
}

type LocalPlayer = (
    With<Player>,
    With<InputMarker<PlayerInput>>,
    With<Predicted>,
);
type OtherPlayer = (
    With<Player>,
    Or<(Without<Predicted>, Without<InputMarker<PlayerInput>>)>,
);

/// What gets drawn for a rider this frame.
#[derive(Component, Default)]
pub struct Rendered {
    pub head: Vec2,
    pub trail: Vec<Vec2>,
}

impl Rendered {
    /// Unit heading along the last non-degenerate trail segment, so the nose
    /// follows the head round a corner; `fallback` when the trail has none.
    pub fn heading(&self, fallback: Vec2) -> Vec2 {
        self.trail
            .windows(2)
            .rev()
            .find_map(|pair| (pair[1] - pair[0]).try_normalize())
            .unwrap_or(fallback)
    }

    fn set(&mut self, head: Vec2, trail: &Trail) {
        self.head = head;
        self.trail.clear();
        self.trail.extend_from_slice(&trail.0);
    }
}

/// Smoothing state of the locally controlled rider.
#[derive(Component)]
pub struct OwnRider {
    tick_start: Vec2,
    prev: Vec2,
    curr: Vec2,
    /// Arc length the drawn head trails the sim head by.
    lag: f32,
    /// Sim head (`curr`) as predicted just before a rollback started.
    predicted_before_rollback: Vec2,
    life: (LifeGeneration, bool),
    teleported: bool,
}

impl OwnRider {
    fn new(position: Vec2, life: (LifeGeneration, bool)) -> Self {
        Self {
            tick_start: position,
            prev: position,
            curr: position,
            lag: 0.0,
            predicted_before_rollback: position,
            life,
            teleported: false,
        }
    }

    /// True on the frame the rider was placed somewhere new (respawn).
    pub(super) fn teleported(&self) -> bool {
        self.teleported
    }
}

/// How far the corrected head moved ahead of where the head was predicted at
/// the same tick, measured along the corrected trail. `None` when the
/// predicted head is no longer on the trail or sits ahead of it: snap.
fn correction_lag(corrected_trail: &[Vec2], predicted: Vec2) -> Option<f32> {
    arc_to_end(corrected_trail, predicted, ON_TRAIL_TOLERANCE)
        .map(|arc| if arc < LAG_EPSILON { 0.0 } else { arc })
}

/// Frame-rate independent decay of the correction lag.
fn decay_lag(lag: f32, dt: f32) -> f32 {
    let lag = lag * (-dt / LAG_DECAY_SECS).exp();
    if lag < LAG_EPSILON { 0.0 } else { lag }
}

/// Fill `out` with the sim trail whose last point is `head`, then cut `lag`
/// off the end. Returns the drawn head.
fn draw_trail(out: &mut Vec<Vec2>, sim_trail: &[Vec2], head: Vec2, lag: f32) -> Vec2 {
    out.clear();
    out.extend_from_slice(sim_trail);
    match out.last_mut() {
        Some(last) => *last = head,
        None => out.push(head),
    }
    truncate_back(out, lag);
    out.last().copied().unwrap_or(head)
}

pub fn add_smoothing(app: &mut App) {
    app.configure_sets(Update, RenderSet::State.before(RenderSet::Draw));
    app.add_systems(FixedPreUpdate, remember_tick_start);
    // Rollback re-runs FixedMain too, so its last tick leaves prev/curr on the
    // corrected path and no tick needs skipping. Both rollback systems only
    // run during a rollback: one reads the predicted head before the state is
    // reset, the other compares it with the corrected head afterwards.
    app.add_systems(FixedPostUpdate, record_tick);
    app.add_systems(
        PreUpdate,
        (
            // Once per frame, before this frame's corrections are added.
            decay_lags.before(RollbackSystems::Check),
            capture_predicted_head.in_set(RollbackSystems::RemoveDisable),
            measure_correction.in_set(RollbackSystems::EndRollback),
        ),
    );
    app.add_observer(drop_lag_on_resync);
    app.add_systems(
        Update,
        (
            update_own_riders,
            update_other_riders.after(InterpolationSystems::Interpolate),
        )
            .in_set(RenderSet::State),
    );
}

fn remember_tick_start(mut riders: Query<(&Position, &mut OwnRider)>) {
    for (position, mut own) in &mut riders {
        own.tick_start = position.0;
    }
}

fn record_tick(mut riders: Query<(&Position, &mut OwnRider)>) {
    for (position, mut own) in &mut riders {
        own.prev = own.tick_start;
        own.curr = position.0;
    }
}

fn decay_lags(time: Res<Time>, mut riders: Query<&mut OwnRider>) {
    for mut own in &mut riders {
        own.lag = decay_lag(own.lag, time.delta_secs());
    }
}

fn capture_predicted_head(mut riders: Query<&mut OwnRider>) {
    for mut own in &mut riders {
        own.predicted_before_rollback = own.curr;
    }
}

/// Runs after the re-simulation: `curr` is now the corrected head of the same
/// tick the prediction had reached, so the difference is the correction alone.
fn measure_correction(mut riders: Query<(&Trail, &mut OwnRider)>) {
    for (trail, mut own) in &mut riders {
        own.lag = match correction_lag(&trail.0, own.predicted_before_rollback) {
            Some(correction) => (own.lag + correction).min(MAX_LAG),
            None => 0.0,
        };
    }
}

/// A timeline resync only renumbers ticks; positions stay valid, so the
/// pending correction is dropped and nothing else changes.
fn drop_lag_on_resync(_: On<SyncEvent<InputTimelineConfig>>, mut riders: Query<&mut OwnRider>) {
    for mut own in &mut riders {
        own.lag = 0.0;
    }
}

#[allow(clippy::type_complexity)]
fn update_own_riders(
    mut commands: Commands,
    fixed: Res<Time<Fixed>>,
    mut riders: Query<
        (
            Entity,
            &Position,
            &Trail,
            &LifeGeneration,
            &IsAlive,
            Option<&mut OwnRider>,
            Option<&mut Rendered>,
        ),
        LocalPlayer,
    >,
) {
    for (entity, position, trail, generation, alive, own, rendered) in &mut riders {
        let life = (*generation, alive.0);
        let (Some(mut own), Some(mut rendered)) = (own, rendered) else {
            let mut first = Rendered::default();
            first.set(position.0, trail);
            commands
                .entity(entity)
                .insert((OwnRider::new(position.0, life), first));
            continue;
        };

        let teleported =
            life.0 != own.life.0 && rendered.head.distance(position.0) > TELEPORT_DISTANCE;
        if own.life != life || !alive.0 {
            *own = OwnRider::new(position.0, life);
            own.teleported = teleported;
            rendered.set(position.0, trail);
            continue;
        }
        own.teleported = false;

        let head = own.prev.lerp(own.curr, fixed.overstep_fraction());
        rendered.head = draw_trail(&mut rendered.trail, &trail.0, head, own.lag);
    }
}

fn update_other_riders(
    mut commands: Commands,
    mut riders: Query<(Entity, &Position, &Trail, Option<&mut Rendered>), OtherPlayer>,
) {
    for (entity, position, trail, rendered) in &mut riders {
        let head = trail.0.last().copied().unwrap_or(position.0);
        match rendered {
            Some(mut rendered) => rendered.set(head, trail),
            None => {
                let mut first = Rendered::default();
                first.set(head, trail);
                commands.entity(entity).insert(first);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trail() -> Vec<Vec2> {
        vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)]
    }

    #[test]
    fn correction_is_the_arc_from_the_predicted_head_to_the_corrected_one() {
        let lag = correction_lag(&trail(), Vec2::new(90.0, 0.0)).unwrap();
        assert!((lag - 10.0).abs() < 1e-3);
    }

    #[test]
    fn rollback_without_error_draws_the_plain_interpolated_head() {
        let sim = trail();
        let predicted = sim[1];
        let lag = correction_lag(&sim, predicted).unwrap();
        assert_eq!(lag, 0.0);
        let head = Vec2::new(97.5, 0.0);
        let mut out = Vec::new();
        assert_eq!(draw_trail(&mut out, &sim, head, lag), head);
        assert_eq!(out, vec![sim[0], head]);
    }

    #[test]
    fn lag_cuts_the_drawn_head_back_along_the_trail() {
        let mut out = Vec::new();
        let head = draw_trail(&mut out, &trail(), Vec2::new(100.0, 0.0), 10.0);
        assert!(head.distance(Vec2::new(90.0, 0.0)) < 1e-3);
    }

    #[test]
    fn sideways_or_overshooting_corrections_snap() {
        assert_eq!(correction_lag(&trail(), Vec2::new(90.0, 20.0)), None);
        assert_eq!(correction_lag(&trail(), Vec2::new(105.0, 0.0)), None);
    }

    #[test]
    fn lag_decays_independent_of_frame_rate() {
        let one_step = decay_lag(10.0, 0.032);
        let two_steps = decay_lag(decay_lag(10.0, 0.016), 0.016);
        assert!((one_step - two_steps).abs() < 1e-3);
        assert_eq!(decay_lag(10.0, 1.0), 0.0);
    }
}
