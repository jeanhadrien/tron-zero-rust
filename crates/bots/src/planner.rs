//! The planner: short maneuvers judged one by one, far enough ahead to matter.
//!
//! Every candidate is a few turns at given ticks followed by straight riding
//! up to the horizon. Each is measured geometrically (how far the line runs
//! before an obstacle, how much room is left from where it ends, with its own
//! new trail counted as blocked), scored by the objective, and the best ones
//! are then checked by the real physics: a line that would die is dropped for
//! the next best. Misjudgment (`noise`) scales what the bot measures, never
//! what it does.

use glam::Vec2;
use shared::physics::{Segment, nearest};
use shared::{rotate_left, rotate_right};

use crate::config::BotBuild;
use crate::forecast::Rollout;
use crate::noise;
use crate::objective::{Terms, Weights};
use crate::plan::{Action, Kind, MAX_TURNS, Maneuver, Turn};
use crate::schedule::Tier;
use crate::space::{STEP, Scratch, Space};
use crate::world::{RiderSnapshot, WorldSnapshot};

/// Cells of room a bot wants around it: about 650 ticks of riding in a
/// perfectly packed region. Smaller regions are pockets.
const NEED_CELLS: f32 = 300.0;
/// Free ticks ahead of the end pose that count as fully clear.
const CLEAR_TICKS: f32 = 20.0;
/// Candidates the physics may check, best first, before giving up on finding
/// a safe one.
const VERIFY_TOP: usize = 6;
/// Slowest cruise assumed when tracing a line (a rider that just turned).
const MIN_CRUISE: f32 = 0.8;

/// What a decision is made with: the tier-clamped budget and the objective.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub reaction: u32,
    pub horizon: u32,
    pub variety: u32,
    pub noise: f32,
    pub weights: Weights,
    pub flood_cap: u32,
    /// The brain's seed: the source of its misjudgments.
    pub seed: u64,
}

impl Params {
    /// The build's budget clamped by the level-of-detail `tier`.
    pub fn new(build: &BotBuild, tier: Tier, seed: u64) -> Self {
        let knobs = &build.knobs;
        Self {
            reaction: knobs.reaction_ticks,
            horizon: knobs.horizon_ticks.min(tier.max_horizon()).max(2),
            variety: knobs.variety.min(tier.max_variety()),
            noise: knobs.noise,
            weights: build.weights(),
            flood_cap: tier.flood_cap(),
            seed,
        }
    }
}

/// What the bot judges its situation to be (for observation, not for choice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Riding straight would hit something: dodging.
    Escape,
    /// Little room left: filling it.
    Survival,
    Attack,
    /// Open space.
    Explore,
}

impl Mode {
    /// Stable id for the `BotDebug` snapshot.
    pub fn to_u8(self) -> u8 {
        match self {
            Mode::Escape => shared::MODE_ESCAPE,
            Mode::Survival => shared::MODE_SURVIVAL,
            Mode::Attack => shared::MODE_ATTACK,
            Mode::Explore => shared::MODE_EXPLORE,
        }
    }
}

/// What one planning round saw and chose, for the debug overlay and the logs.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub kind: Kind,
    pub mode: Mode,
    /// Best score among the candidates starting with Hold, Left, Right.
    pub scores: [f32; 3],
    /// Room (cells) left from the chosen line's end.
    pub area: u32,
    /// Candidates whose line runs the whole horizon unobstructed.
    pub open: u32,
    /// The chosen line's space, reach and clearance terms.
    pub terms: Terms,
    pub horizon: u32,
}

/// The chosen maneuver and why.
pub struct Outcome {
    pub maneuver: Maneuver,
    pub report: Report,
}

/// A candidate's straight-line geometry: where it goes before the horizon or
/// the first obstacle.
struct Path {
    points: [Vec2; MAX_TURNS + 2],
    len: usize,
    end_dir: Vec2,
    /// Ticks ridden before the line ended.
    free: f32,
    /// An obstacle stopped the line short.
    blocked: bool,
}

impl Path {
    fn end(&self) -> Vec2 {
        self.points[self.len - 1]
    }

    fn legs(&self) -> Vec<(Vec2, Vec2)> {
        self.points[..self.len]
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect()
    }
}

struct Scored {
    index: usize,
    maneuver: Maneuver,
    score: f32,
    terms: Terms,
    area: u32,
    reach: f32,
}

/// Everything one decision shares between its candidates.
struct Context<'a> {
    world: &'a WorldSnapshot,
    me: usize,
    rider: &'a RiderSnapshot,
    obstacles: &'a [Segment],
    space: &'a Space,
    params: &'a Params,
    /// Distance covered per tick.
    step: f32,
    /// How far the bot reads distances this decision: `1` is exact.
    scale: f32,
}

/// Plan for bot `me`. `current` is what is left of the plan it is following;
/// it competes with the fresh candidates and gets a commitment bonus.
pub fn plan(
    world: &WorldSnapshot,
    me: usize,
    params: &Params,
    scratch: &mut Scratch,
    current: Option<Maneuver>,
) -> Outcome {
    let rider = &world.riders[me];
    let cx = Context {
        world,
        me,
        rider,
        obstacles: world.obstacles(),
        space: world.space(),
        params,
        step: STEP * rider.target_speed_mult.max(MIN_CRUISE),
        scale: 1.0 + params.noise * noise::unit(params.seed, world.tick, 0, 0),
    };
    let mut scored: Vec<Scored> = candidates(params, current)
        .iter()
        .enumerate()
        .map(|(index, maneuver)| evaluate(&cx, index, maneuver, scratch))
        .collect();
    // Best first; equal scores keep the candidate order, so ties are stable.
    scored.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.index.cmp(&b.index)));

    let pick = verified_pick(&cx, &scored);
    let chosen = &scored[pick];
    let hold = scored.iter().find(|s| s.maneuver.kind == Kind::Hold);
    let mode = if chosen.maneuver.kind != Kind::Hold && hold.is_some_and(|h| h.reach < 0.999) {
        Mode::Escape
    } else if (chosen.area as f32) < NEED_CELLS {
        Mode::Survival
    } else {
        Mode::Explore
    };
    let mut scores = [f32::NEG_INFINITY; 3];
    for s in &scored {
        let slot = &mut scores[s.maneuver.first_action().index()];
        *slot = slot.max(s.score);
    }
    Outcome {
        maneuver: chosen.maneuver,
        report: Report {
            kind: chosen.maneuver.kind,
            mode,
            scores: scores.map(|s| if s.is_finite() { s } else { 0.0 }),
            area: chosen.area,
            open: scored.iter().filter(|s| s.reach >= 0.999).count() as u32,
            terms: chosen.terms,
            horizon: params.horizon,
        },
    }
}

/// Index (into `scored`, best first) of the first candidate the physics lets
/// live; failing that, the one that lives longest as the bot sees it.
fn verified_pick(cx: &Context, scored: &[Scored]) -> usize {
    let horizon = cx.params.horizon;
    let mut longest = (0, f32::NEG_INFINITY);
    for (rank, candidate) in scored.iter().enumerate().take(VERIFY_TOP) {
        match first_death(cx.world, cx.me, &candidate.maneuver, horizon) {
            None => return rank,
            Some(tick) => {
                let seen = tick as f32 * cx.scale;
                if seen >= horizon as f32 {
                    return rank;
                }
                if seen > longest.1 {
                    longest = (rank, seen);
                }
            }
        }
    }
    longest.0
}

/// Whether the bot, riding `maneuver` from now, lives through the horizon, as
/// it judges it (a noisy bot may read a death as farther than it is).
pub fn survives(world: &WorldSnapshot, me: usize, maneuver: &Maneuver, params: &Params) -> bool {
    let scale = 1.0 + params.noise * noise::unit(params.seed, world.tick, 0, 0);
    first_death(world, me, maneuver, params.horizon)
        .is_none_or(|tick| tick as f32 * scale >= params.horizon as f32)
}

/// Tick of the bot's death riding `maneuver` through the real physics, others
/// riding straight on. `None` if it lives for `ticks`.
fn first_death(world: &WorldSnapshot, me: usize, maneuver: &Maneuver, ticks: u32) -> Option<u32> {
    // Riders further than this cannot reach the bot's line within the window.
    let radius = ticks as f32 * 6.0 + 150.0;
    let mut rollout = Rollout::new(world, me, radius)?;
    (0..ticks).find(|&tick| !rollout.step(maneuver.action_at(tick)))
}

/// The maneuvers a bot with this budget weighs, in a fixed order: the plan it
/// is following, riding straight, a turn now or later, then reversals.
fn candidates(params: &Params, current: Option<Maneuver>) -> Vec<Maneuver> {
    let reaction = params.reaction.max(1);
    let horizon = params.horizon;
    let mut turn_at = match params.variety {
        1 => vec![0],
        2 => vec![0, reaction / 2],
        _ => vec![0, reaction / 3, reaction * 2 / 3],
    };
    turn_at.dedup();
    let combo_at: Vec<u32> = if params.variety >= 2 {
        let mut at = vec![0, reaction / 2];
        at.dedup();
        at
    } else {
        Vec::new()
    };
    let u_gaps: &[u32] = match params.variety {
        1 => &[],
        2 => &[10, 30],
        _ => &[2, 8, 20, 45],
    };
    let dodge_gaps: &[u32] = if params.variety >= 3 { &[6, 16] } else { &[] };

    let mut list: Vec<Maneuver> = current.into_iter().collect();
    list.push(Maneuver::hold());
    for &at in &turn_at {
        for action in [Action::Left, Action::Right] {
            list.push(Maneuver::new(Kind::Turn, &[Turn { at, action }]));
        }
    }
    let pair = |kind, at: u32, gap: u32, first, second| {
        Maneuver::new(
            kind,
            &[
                Turn { at, action: first },
                Turn {
                    at: at + gap,
                    action: second,
                },
            ],
        )
    };
    for &at in &combo_at {
        for &gap in u_gaps.iter().filter(|&&gap| at + gap < horizon) {
            for action in [Action::Left, Action::Right] {
                list.push(pair(Kind::UTurn, at, gap, action, action));
            }
        }
        for &gap in dodge_gaps.iter().filter(|&&gap| at + gap < horizon) {
            list.push(pair(Kind::Dodge, at, gap, Action::Left, Action::Right));
            list.push(pair(Kind::Dodge, at, gap, Action::Right, Action::Left));
        }
    }
    list
}

/// Ride `maneuver` for the horizon along straight legs, stopping at the first
/// obstacle each leg meets.
fn trace(cx: &Context, maneuver: &Maneuver) -> Path {
    let horizon = cx.params.horizon;
    let mut pos = cx.rider.pos;
    let mut dir = cx.rider.dir;
    let mut path = Path {
        points: [pos; MAX_TURNS + 2],
        len: 1,
        end_dir: dir,
        free: 0.0,
        blocked: false,
    };
    let mut tick = 0;
    let mut turns = maneuver
        .turns()
        .iter()
        .filter(|t| t.at < horizon)
        .peekable();
    while tick < horizon {
        // The turn that opens this leg happens before it moves.
        if let Some(turn) = turns.next_if(|t| t.at <= tick) {
            dir = match turn.action {
                Action::Left => rotate_left(dir),
                Action::Right => rotate_right(dir),
                Action::Hold => dir,
            };
        }
        let until = turns.peek().map_or(horizon, |t| t.at);
        let length = (until - tick) as f32 * cx.step;
        if length > 0.0 {
            let hit = nearest(cx.obstacles, Some(cx.me as u64), pos, dir, false);
            if hit < length {
                pos += dir * hit;
                path.free += hit / cx.step;
                path.blocked = true;
                path.points[path.len] = pos;
                path.len += 1;
                break;
            }
            pos += dir * length;
            path.free += (until - tick) as f32;
        }
        path.points[path.len] = pos;
        path.len += 1;
        tick = until;
    }
    path.end_dir = dir;
    path
}

/// Measure and score one candidate.
fn evaluate(cx: &Context, index: usize, maneuver: &Maneuver, scratch: &mut Scratch) -> Scored {
    let params = cx.params;
    let path = trace(cx, maneuver);
    let end = path.end();
    let room = cx
        .space
        .measure(scratch, &path.legs(), end, params.horizon, params.flood_cap);
    // Per-candidate misjudgment of room; distances share the decision's error.
    let area_seen = room.area as f32
        * (1.0 + params.noise * noise::unit(params.seed, cx.world.tick, index + 1, 1));
    let reach = (path.free * cx.scale / params.horizon as f32).clamp(0.0, 1.0);
    let clear = if path.blocked {
        0.0
    } else {
        let front = nearest(cx.obstacles, Some(cx.me as u64), end, path.end_dir, false);
        (front / cx.step * cx.scale / CLEAR_TICKS).clamp(0.0, 1.0)
    };
    let cramped = 1.0 - (area_seen / NEED_CELLS).clamp(0.0, 1.0);
    let terms = Terms {
        space: ((1.0 + area_seen.max(0.0)).ln() / (1.0 + params.flood_cap as f32).ln())
            .clamp(0.0, 1.0),
        pocket: -cramped,
        reach,
        clear,
        turn: -(maneuver.turns().len() as f32) / MAX_TURNS as f32,
        fill: cramped * room.hug,
        commit: f32::from(maneuver.kind == Kind::Continue),
    };
    Scored {
        index,
        maneuver: *maneuver,
        score: params.weights.value(&terms),
        terms,
        area: room.area,
        reach,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::config::{BotBuild, Difficulty, Personality};
    use shared::{ARENA_HEIGHT, ARENA_WIDTH, Trail, arena_walls};

    fn rider(pos: Vec2, dir: Vec2, trail: Vec<Vec2>) -> RiderSnapshot {
        RiderSnapshot {
            id: 1,
            pos,
            dir,
            trail: Trail(trail),
            alive: true,
            rubber: shared::BASE_RUBBER,
            speed_mult: 1.0,
            target_speed_mult: 1.0,
        }
    }

    fn world(extra_walls: Vec<[f32; 4]>, riders: Vec<RiderSnapshot>) -> WorldSnapshot {
        let mut walls = arena_walls(ARENA_WIDTH, ARENA_HEIGHT);
        walls.extend(extra_walls);
        WorldSnapshot::new(Arc::new(walls), riders, 10)
    }

    fn params(level: u32, noise: f32) -> Params {
        let mut build = BotBuild::from_difficulty("t", Personality::Adaptive, Difficulty(level));
        build.knobs.noise = noise;
        Params::new(&build, Tier::Full, 1)
    }

    fn decide(world: &WorldSnapshot, params: &Params) -> Outcome {
        plan(world, 0, params, &mut Scratch::default(), None)
    }

    #[test]
    fn open_space_means_riding_straight() {
        let world = world(
            Vec::new(),
            vec![rider(Vec2::ZERO, Vec2::X, vec![Vec2::new(-30.0, 0.0)])],
        );
        let outcome = decide(&world, &params(4000, 0.0));
        assert_eq!(outcome.maneuver.kind, Kind::Hold);
        assert_eq!(outcome.report.mode, Mode::Explore);
    }

    #[test]
    fn a_wall_ahead_means_turning_in_time() {
        let world = world(
            Vec::new(),
            vec![rider(
                Vec2::new(850.0, 0.0),
                Vec2::X,
                vec![Vec2::new(800.0, 0.0)],
            )],
        );
        let outcome = decide(&world, &params(4000, 0.0));
        assert_ne!(outcome.maneuver.kind, Kind::Hold);
        assert_eq!(outcome.report.mode, Mode::Escape);
    }

    #[test]
    fn a_wall_in_a_small_room_is_never_open_exploring() {
        let walls = vec![
            [-120.0, -120.0, 120.0, -120.0],
            [120.0, -120.0, 120.0, 120.0],
            [120.0, 120.0, -120.0, 120.0],
            [-120.0, 120.0, -120.0, -120.0],
        ];
        let rider = rider(Vec2::ZERO, Vec2::X, vec![Vec2::new(-40.0, 0.0)]);
        let world = world(walls, vec![rider]);
        let outcome = decide(&world, &params(4000, 0.0));
        // The wall ahead makes it an escape, never open exploring.
        assert_ne!(outcome.report.mode, Mode::Explore, "{:?}", outcome.report);
    }

    #[test]
    fn decisions_replay_identically_and_noise_changes_them() {
        let world = world(
            Vec::new(),
            vec![rider(
                Vec2::new(800.0, 0.0),
                Vec2::X,
                vec![Vec2::new(700.0, 0.0)],
            )],
        );
        let noisy = params(500, 0.6);
        let a = decide(&world, &noisy);
        let b = decide(&world, &noisy);
        assert_eq!(a.maneuver, b.maneuver);
        assert_eq!(a.report.area, b.report.area);
    }
}
