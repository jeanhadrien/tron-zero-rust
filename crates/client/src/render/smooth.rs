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
//! * Opponents: the replicated `Trail` holds corners only and `Position` steps
//!   tick to tick, so the trail is rebuilt from the two confirmed snapshots
//!   around the interpolation time and its head slides between them
//!   (`shared::polyline::interpolate_trail`); the body is drawn from its head.

use bevy::prelude::*;
use lightyear::prelude::client::InputTimelineConfig;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{
    ConfirmedHistory, InterpolationSystems, InterpolationTimeline, NetworkTimeline, Predicted,
    RollbackSystems, SyncEvent, Tick, interpolation_fraction,
};
use shared::polyline::{arc_to_end, interpolate_trail, truncate_back};
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

    /// The sim trail as is, with the rider at `head` (snaps, opponents'
    /// fallback). Fills the existing buffer.
    fn snap(&mut self, trail: &Trail, head: Vec2) {
        self.head = head;
        trail.points_into(head, &mut self.trail);
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

/// Move the last point of the sim trail in `trail` to `head`, then cut `lag`
/// off the end. Returns the drawn head.
fn draw_head(trail: &mut Vec<Vec2>, head: Vec2, lag: f32) -> Vec2 {
    match trail.last_mut() {
        Some(last) => *last = head,
        None => trail.push(head),
    }
    truncate_back(trail, lag);
    trail.last().copied().unwrap_or(head)
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
fn measure_correction(mut riders: Query<(&Trail, &Position, &mut OwnRider)>) {
    for (trail, position, mut own) in &mut riders {
        let corrected = trail.points(position.0);
        own.lag = match correction_lag(&corrected, own.predicted_before_rollback) {
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
            first.snap(trail, position.0);
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
            rendered.snap(trail, position.0);
            continue;
        }
        own.teleported = false;

        let head = own.prev.lerp(own.curr, fixed.overstep_fraction());
        trail.points_into(position.0, &mut rendered.trail);
        rendered.head = draw_head(&mut rendered.trail, head, own.lag);
    }
}

fn update_other_riders(
    mut commands: Commands,
    timeline: Query<&InterpolationTimeline>,
    mut riders: Query<
        (
            Entity,
            &Position,
            &Trail,
            Option<&ConfirmedHistory<Position>>,
            Option<&ConfirmedHistory<Trail>>,
            Option<&mut Rendered>,
        ),
        OtherPlayer,
    >,
) {
    let now = timeline
        .single()
        .ok()
        .map(|timeline| (timeline.tick(), timeline.overstep().to_f32()));
    for (entity, position, trail, heads, trails, rendered) in &mut riders {
        let interpolated = match (now, heads, trails) {
            (Some((tick, overstep)), Some(heads), Some(trails)) => {
                interpolated_points(heads, trails, tick, overstep)
            }
            _ => None,
        };
        let mut fresh = None;
        let rendered = match rendered {
            Some(rendered) => rendered.into_inner(),
            None => fresh.insert(Rendered::default()),
        };
        match interpolated {
            Some(points) => {
                rendered.head = points.last().copied().unwrap_or(position.0);
                rendered.trail = points;
            }
            None => rendered.snap(trail, position.0),
        }
        if let Some(fresh) = fresh {
            commands.entity(entity).insert(fresh);
        }
    }
}

/// An opponent's trail at the interpolation time `tick + overstep`: rebuilt
/// from the confirmed snapshots just before and after it (corners and head of
/// the same tick), the head sliding along the newer one. Mirrors Lightyear's
/// `InterpolationRegistry::sample` (crate-private): the same bracketing, no
/// extrapolation past the newest snapshot. `None` before the first snapshot.
fn interpolated_points(
    heads: &ConfirmedHistory<Position>,
    trails: &ConfirmedHistory<Trail>,
    tick: Tick,
    overstep: f32,
) -> Option<Vec<Vec2>> {
    let before = (0..heads.len())
        .take_while(|&i| heads.get_nth_tick(i).is_some_and(|t| t <= tick))
        .last()?;
    let (start_tick, start_head) = heads.get_nth_present(before)?;
    let start = trails.get_present(start_tick)?.points(start_head.0);
    let end = heads
        .get_nth_present(before + 1)
        .and_then(|(end_tick, end_head)| {
            let end = trails.get_present(end_tick)?.points(end_head.0);
            Some((end_tick, end))
        });
    let Some((end_tick, end)) = end else {
        return Some(start);
    };
    let t = interpolation_fraction(start_tick, end_tick, tick, overstep).clamp(0.0, 1.0);
    Some(interpolate_trail(&start, &end, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear::prelude::HistoryState;

    fn trail() -> Vec<Vec2> {
        vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)]
    }

    /// Confirmed opponent snapshots: `(tick, corners, head)`.
    fn histories(
        snapshots: &[(u32, Vec<Vec2>, Vec2)],
    ) -> (ConfirmedHistory<Position>, ConfirmedHistory<Trail>) {
        let mut heads = ConfirmedHistory::default();
        let mut trails = ConfirmedHistory::default();
        for (tick, corners, head) in snapshots {
            heads.insert_explicit(Tick(*tick), HistoryState::Updated(Position(*head)));
            trails.insert_explicit(Tick(*tick), HistoryState::Updated(Trail(corners.clone())));
        }
        (heads, trails)
    }

    #[test]
    fn opponent_head_slides_through_a_corner_between_snapshots() {
        let (heads, trails) = histories(&[
            (10, vec![Vec2::ZERO], Vec2::new(8.0, 0.0)),
            (
                11,
                vec![Vec2::ZERO, Vec2::new(10.0, 0.0)],
                Vec2::new(10.0, 4.0),
            ),
        ]);
        // 6 units travelled; half of it undone leaves the head at (10, 1).
        let points = interpolated_points(&heads, &trails, Tick(10), 0.5).unwrap();
        assert_eq!(points.len(), 3);
        assert!(points[2].distance(Vec2::new(10.0, 1.0)) < 1e-3);
        let end = interpolated_points(&heads, &trails, Tick(11), 0.0).unwrap();
        assert_eq!(
            end,
            trails
                .newest_present()
                .unwrap()
                .1
                .points(Vec2::new(10.0, 4.0))
        );
    }

    #[test]
    fn opponent_respawn_steps_and_a_missing_end_holds() {
        let start = vec![Vec2::ZERO, Vec2::new(8.0, 0.0)];
        let (heads, trails) = histories(&[
            (10, vec![Vec2::ZERO], Vec2::new(8.0, 0.0)),
            (11, vec![Vec2::splat(500.0)], Vec2::splat(500.0)),
        ]);
        assert_eq!(
            interpolated_points(&heads, &trails, Tick(10), 0.9),
            Some(start.clone())
        );
        let (heads, trails) = histories(&[(10, vec![Vec2::ZERO], Vec2::new(8.0, 0.0))]);
        assert_eq!(
            interpolated_points(&heads, &trails, Tick(12), 0.5),
            Some(start)
        );
        assert_eq!(interpolated_points(&heads, &trails, Tick(9), 0.5), None);
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
        let mut out = sim.clone();
        assert_eq!(draw_head(&mut out, head, lag), head);
        assert_eq!(out, vec![sim[0], head]);
    }

    #[test]
    fn lag_cuts_the_drawn_head_back_along_the_trail() {
        let head = draw_head(&mut trail(), Vec2::new(100.0, 0.0), 10.0);
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
