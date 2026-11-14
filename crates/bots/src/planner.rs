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
use shared::physics::{Segment, nearest, normal_step, ray_hit, rubber_zone};
use shared::{BASE_RUBBER, GRIND_RANGE, RUBBER_APPROACH, RUBBER_COST, rotate_left, rotate_right};

use crate::config::BotBuild;
use crate::forecast::Rollout;
use crate::grind::{self, Speed};
use crate::noise;
use crate::objective::{Terms, Weights};
use crate::plan::{Action, Kind, MAX_TURNS, Maneuver, Turn};
use crate::race::{self, Cells, Laid, MAX_RACERS};
use crate::schedule::Tier;
use crate::space::{CELL, COARSE, NEED_CELLS, STEP, Scratch, Space};
use crate::style::{Figure, Progress};
use crate::world::{RiderSnapshot, WorldSnapshot};

/// Free ticks ahead of the end pose that count as fully clear.
const CLEAR_TICKS: f32 = 20.0;
/// Candidates the physics may check, best first, before giving up on finding
/// a safe one.
const VERIFY_TOP: usize = 6;
/// Slowest cruise assumed when tracing a line (a rider that just turned).
const MIN_CRUISE: f32 = 0.8;
/// Candidates (best first, plus riding straight) that get the territory race.
const RACE_TOP: usize = 7;
/// Opponents closer than this need no engagement.
const ENGAGE_RANGE: f32 = 250.0;
/// Further than `ENGAGE_RANGE` by this much, engagement is at its worst.
const ENGAGE_SPAN: f32 = 1200.0;
/// Engagement tells headings apart in steps of `1 / ENGAGE_STEPS` of cosine.
const ENGAGE_STEPS: f32 = 3.0;
/// Ticks of a bot's life for engagement to count one more time over.
const ENGAGE_RAMP: f32 = 600.0;
/// Engagement never counts more than this many times its weight.
const ENGAGE_MAX: f32 = 4.0;
/// Territory (coarse cells) under which an opponent is sealed in a pocket.
const SEAL_CELLS: f32 = NEED_CELLS / (COARSE * COARSE) as f32;
/// How much territory is worth in the choice of a target: a target counts as
/// this many logs of cells closer for every 1000 units it is away.
const TARGET_DISTANCE: f32 = 1.2;
/// How many logs of cells a target with all its rubber burned is worth.
const TARGET_RUBBER: f32 = 1.0;
/// A new target must beat the current one by this much (logs of cells).
const TARGET_STICK: f32 = 0.6;
/// Ticks ahead the race looks: every rider is taken to ride straight on after
/// its maneuver until then (or an obstacle), laying trail as it goes.
const RACE_TICKS: f32 = 160.0;
/// Pressure a line below the calm risk window needs to be worth the risk: it
/// has to seal the target, cut it off or squeeze it hard.
const BOLD_PRESSURE: f32 = 1.0;
/// Heat at which a bold line needs no kill chance at all.
const BOLD_FADE: f32 = 6.0;
/// A round's heat is full `HEAT_SPAN` ticks after the round began, and keeps
/// rising to `HEAT_MAX`. Only bots told they play a round feel it.
const HEAT_SPAN: f32 = 500.0;
const HEAT_MAX: f32 = 3.0;
/// Riders further than `ROLLOUT_REACH` units per tick of the window, plus
/// `ROLLOUT_MARGIN`, are left out of a physics check: they cannot reach the
/// bot's line in time.
const ROLLOUT_REACH: f32 = 6.0;
const ROLLOUT_MARGIN: f32 = 150.0;
/// Territory only matters to a bot with an opponent this close (units); the
/// weight fades out over `CONTACT_SPAN` further.
const CONTACT_NEAR: f32 = 900.0;
const CONTACT_SPAN: f32 = 1200.0;
/// Best lines (after the race) whose effect on the target's replies is
/// checked, and how close the target must be for it.
const REPLY_TOP: usize = 3;
const REPLY_RANGE: f32 = 700.0;
/// What the target may do in reply: turn now or later, or ride straight.
const REPLIES: [(u32, Action); 6] = [
    (0, Action::Left),
    (0, Action::Right),
    (6, Action::Left),
    (6, Action::Right),
    (18, Action::Left),
    (18, Action::Right),
];
/// A cut across the foe's line needs at least this many ticks to spare, counts
/// in full up to `CUT_BEST_MARGIN`, and fades out over `CUT_FADE` more.
const CUT_MIN_MARGIN: f32 = 3.0;
/// How far ahead (ticks) the bot's line is followed when looking for a cut.
const CUT_TICKS: f32 = 450.0;
const CUT_BEST_MARGIN: f32 = 30.0;
const CUT_FADE: f32 = 150.0;
/// Pressure at which a line counts as an attack.
const ATTACK_PRESSURE: f32 = 0.6;
/// Speed (x base) beyond which more is not worth a detour: the bot has what it
/// needs to hunt.
const GRIND_CAP: f32 = 1.5;
/// How much an opponent's speed against the bot's scales engagement (min, max).
const ENGAGE_PACE: (f32, f32) = (0.5, 2.0);
/// How close beside an obstacle a grind line is aimed (units).
const GRIND_GAP: f32 = 10.0;
/// Farthest side obstacle (units) a lane towards it is worth planning.
const LANE_REACH: f32 = 420.0;
/// Margins (units) a late turn ahead of an obstacle is aimed at.
const LATE_GAPS: [f32; 2] = [14.0, 36.0];
/// Rubber kept in reserve when the burn is judged against what is left.
const RUBBER_SLACK: f32 = 10.0;

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
    /// Ticks since the brain began to look: how long the round has lasted for
    /// this bot.
    pub age: u32,
    /// The bot plays last rider standing over rounds: the round heats up.
    pub rounds: bool,
    /// The last plan was to ride straight on: riding straight still is
    /// following it.
    pub steady: bool,
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
            weights: build.weights,
            flood_cap: tier.flood_cap(),
            seed,
            age: 0,
            rounds: false,
            steady: false,
        }
    }
}

/// How hot the round is for the bot, `0..=HEAT_MAX` (`1` is full): a round that
/// nobody wins by waiting has to come to an end, so riders grow bolder as it ages.
fn heat(params: &Params) -> f32 {
    if !params.rounds {
        return 0.0;
    }
    (params.age as f32 / HEAT_SPAN).clamp(0.0, HEAT_MAX)
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
    /// The opponent being pressured (entity bits), if any.
    pub target: Option<u64>,
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

    /// Where to start measuring room from: the end, or a little before it
    /// when an obstacle stopped the line (the room is on this side of it).
    fn flood_start(&self) -> Vec2 {
        let end = self.end();
        if !self.blocked || self.len < 2 {
            return end;
        }
        let leg = end - self.points[self.len - 2];
        end - leg.clamp_length_max(CELL * 0.75)
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
    path: Path,
    maneuver: Maneuver,
    score: f32,
    terms: Terms,
    area: u32,
    reach: f32,
    /// How safe the line is to fight on, `0..=1`: fighting terms are scaled by
    /// it, so a bot never pays for a pocket or a collision with a kill.
    safe: f32,
    /// The cruise speed the line reaches by grinding.
    speed: Speed,
    /// Within the bot's risk of the best survival score: the fight terms may
    /// choose it. Ranked above every line that is not.
    eligible: bool,
    /// Eligible only thanks to the round's heat: below the calm risk window, so
    /// it must buy a real kill chance (`BOLD_PRESSURE`) to stay eligible.
    bold: bool,
}

/// Everything one decision shares between its candidates.
struct Context<'a> {
    world: &'a WorldSnapshot,
    me: usize,
    rider: &'a RiderSnapshot,
    obstacles: &'a [Segment],
    /// The obstacles that are not the bot's own trail: what it may grind. A bot
    /// that hugs its own coil only boxes itself in.
    others: Vec<Segment>,
    space: &'a Space,
    params: &'a Params,
    /// Distance covered per tick.
    step: f32,
    /// How far the bot reads distances this decision: `1` is exact.
    scale: f32,
    /// The living opponents, nearest first.
    foes: Vec<Foe>,
    /// Engagement's weight in this decision: it grows with the round.
    ramp: f32,
    /// The objective of this decision: the build's, worn down by the round.
    weights: Weights,
    /// The bot's figure and where it stands in it, for a bot with style.
    figure: Option<(Figure, Progress)>,
}

/// An opponent as the bot sees it: where it is and where it is likely to be.
struct Foe {
    index: usize,
    id: u64,
    pos: Vec2,
    /// Where it ends up riding straight on until an obstacle or the horizon.
    future: Vec2,
    /// The same, until the end of the race's lookahead.
    line_end: Vec2,
    /// Distance it covers per tick.
    pace: f32,
    /// Rubber it has left: a rider that has burned it is prey.
    rubber: f32,
    /// How far it can ride straight before an obstacle.
    clear: f32,
    /// Ticks it needs to cross a coarse cell.
    cost: u16,
}

/// What a decision of bot `me` shares between its candidates.
fn context<'a>(world: &'a WorldSnapshot, me: usize, params: &'a Params) -> Context<'a> {
    let rider = &world.riders[me];
    let mut cx = Context {
        world,
        me,
        rider,
        obstacles: world.obstacles(),
        others: world
            .obstacles()
            .iter()
            .filter(|s| s.owner != Some(me as u64))
            .copied()
            .collect(),
        space: world.space(),
        params,
        step: STEP * rider.target_speed_mult.max(MIN_CRUISE),
        scale: 1.0 + params.noise * noise::unit(params.seed, world.tick, 0, 0),
        foes: Vec::new(),
        ramp: 1.0 + (params.age as f32 / ENGAGE_RAMP).min(ENGAGE_MAX - 1.0),
        weights: params.weights,
        figure: Figure::of(&params.weights)
            .map(|figure| (figure, figure.progress(&rider.trail.points(rider.pos)))),
    };
    cx.foes = foes(&cx);
    // An artist does not take up the fight as the round ages.
    if !cx.foes.is_empty() && cx.figure.is_none() {
        cx.weights = params.weights.heated(heat(params));
    }
    cx
}

/// Plan for bot `me`. `current` is what is left of the plan it is following;
/// it competes with the fresh candidates and gets a commitment bonus. `focus`
/// is the opponent it pressured last time, kept unless another is clearly
/// more vulnerable.
pub fn plan(
    world: &WorldSnapshot,
    me: usize,
    params: &Params,
    scratch: &mut Scratch,
    current: Option<Maneuver>,
    focus: Option<u64>,
) -> Outcome {
    let cx = context(world, me, params);
    let mut scored: Vec<Scored> = candidates(&cx, current)
        .iter()
        .enumerate()
        .map(|(index, maneuver)| evaluate(&cx, index, maneuver, scratch))
        .collect();
    // Best first; equal scores keep the candidate order, so ties are stable.
    sort(&mut scored);
    let target = if params.weights.fights() && !cx.foes.is_empty() {
        fight(&cx, &mut scored, scratch, focus)
    } else {
        None
    };

    let pick = verified_pick(&cx, &scored);
    let chosen = &scored[pick];
    let hold = scored.iter().find(|s| s.maneuver.kind == Kind::Hold);
    let mode = if chosen.maneuver.kind != Kind::Hold && hold.is_some_and(|h| h.reach < 0.999) {
        Mode::Escape
    } else if (chosen.area as f32) < NEED_CELLS {
        Mode::Survival
    } else if chosen.terms.pressure >= ATTACK_PRESSURE {
        Mode::Attack
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
            target,
        },
    }
}

fn sort(scored: &mut [Scored]) {
    scored.sort_by(|a, b| {
        (b.eligible.cmp(&a.eligible))
            .then(b.score.total_cmp(&a.score))
            .then(a.index.cmp(&b.index))
    });
}

/// Ticks a rider at `speed` needs to cross one coarse cell.
fn cell_cost(speed: f32) -> u16 {
    (CELL * COARSE as f32 / (STEP * speed.max(MIN_CRUISE)))
        .round()
        .max(1.0) as u16
}

/// The living opponents with their likely whereabouts, nearest first, at most
/// as many as a race holds.
fn foes(cx: &Context) -> Vec<Foe> {
    let me = cx.rider.pos;
    let horizon = cx.params.horizon as f32;
    let mut list: Vec<Foe> = cx
        .world
        .riders
        .iter()
        .enumerate()
        .filter(|&(i, r)| i != cx.me && r.alive)
        .map(|(index, r)| {
            let speed = r.speed_mult.max(r.target_speed_mult);
            let pace = STEP * speed.max(MIN_CRUISE);
            let hit = nearest(cx.obstacles, Some(index as u64), r.pos, r.dir, false);
            Foe {
                index,
                id: r.id,
                pos: r.pos,
                future: r.pos + r.dir * hit.min(horizon * pace),
                line_end: r.pos + r.dir * hit.min(RACE_TICKS * pace),
                pace,
                rubber: r.rubber,
                clear: hit,
                cost: cell_cost(speed),
            }
        })
        .collect();
    list.sort_by(|a, b| {
        me.distance_squared(a.pos)
            .total_cmp(&me.distance_squared(b.pos))
            .then(a.index.cmp(&b.index))
    });
    list.truncate(MAX_RACERS - 1);
    list
}

/// How much territory matters to a bot with the nearest opponent `near` units
/// away: all of it close by, nothing far off.
fn contact(near: f32) -> f32 {
    (1.0 - (near - CONTACT_NEAR) / CONTACT_SPAN).clamp(0.0, 1.0)
}

/// Bring the fight terms into the choice, among the lines nearly as good as
/// the best for survival (within the bot's risk): engagement for all of them,
/// and when an opponent is close, the territory race. Returns the target.
fn fight(
    cx: &Context,
    scored: &mut [Scored],
    scratch: &mut Scratch,
    focus: Option<u64>,
) -> Option<u64> {
    let best = scored[0].score;
    let (floor, calm) = (best - cx.weights.risk, best - cx.params.weights.risk);
    for line in scored.iter_mut() {
        line.eligible = line.score >= floor;
        line.bold = line.score < calm;
        if line.eligible {
            line.score = cx.weights.value(&line.terms);
        }
    }
    sort(scored);
    let target = if cx.weights.races() {
        race_top(cx, scored, scratch, focus)
    } else {
        None
    };
    // Risk beyond the calm window is only taken for a kill chance, and the
    // later the round the smaller the chance asked for.
    let need = BOLD_PRESSURE * (1.0 - heat(cx.params) / BOLD_FADE).max(0.0);
    for line in scored.iter_mut() {
        if line.bold && line.terms.pressure < need {
            line.eligible = false;
        }
    }
    sort(scored);
    target
}

/// Run the territory race for the best candidates (and riding straight),
/// pick whom to pressure, and fold the race's terms into their scores. The
/// race terms never go below zero, so a refined candidate never falls behind
/// one left unrefined. Returns the target, if the race ran.
fn race_top(
    cx: &Context,
    scored: &mut [Scored],
    scratch: &mut Scratch,
    focus: Option<u64>,
) -> Option<u64> {
    let me = cx.rider.pos;
    // Far from every opponent, territory says nothing but "stay central".
    let contact = contact(cx.foes.first()?.pos.distance(me));
    if contact <= 0.0 {
        return None;
    }
    let mut picks: Vec<usize> = (0..scored.len().min(RACE_TOP)).collect();
    picks.retain(|&i| scored[i].eligible);
    if let Some(hold) = scored.iter().position(|s| s.maneuver.kind == Kind::Hold)
        && !picks.contains(&hold)
    {
        picks.push(hold);
    }
    let foe_costs: Vec<u16> = cx.foes.iter().map(|foe| foe.cost).collect();
    let foe_lines: Vec<Laid> = cx
        .foes
        .iter()
        .enumerate()
        .map(|(n, foe)| Laid {
            owner: n + 1,
            from: foe.pos,
            to: foe.line_end,
            tick_from: 0.0,
            tick_to: foe.pos.distance(foe.line_end) / foe.pace,
        })
        .collect();
    let results: Vec<(usize, Cells, Vec<Laid>)> = picks
        .iter()
        .filter(|&&i| !scored[i].path.blocked)
        .map(|&i| {
            let path = &scored[i].path;
            // Grinding on the way changes how fast the line is ridden, and how
            // fast the bot rides on after it.
            let speed = scored[i].speed;
            let (pace, onward) = (
                STEP * speed.mean.max(MIN_CRUISE),
                STEP * speed.end.max(MIN_CRUISE),
            );
            let mut costs = vec![cell_cost(speed.end)];
            costs.extend(&foe_costs);
            let mut lines = laid_legs(path, pace);
            // The bot rides straight on after its maneuver.
            let end = path.end();
            let ahead = nearest(cx.obstacles, Some(cx.me as u64), end, path.end_dir, false)
                .min((RACE_TICKS - path.free).max(0.0) * onward);
            let last = lines.last().map_or(0.0, |leg| leg.tick_to);
            lines.push(Laid {
                owner: 0,
                from: end,
                to: end + path.end_dir * ahead,
                tick_from: last,
                tick_to: last + ahead / onward,
            });
            // The cut-off looks further along the same line.
            let far = nearest(cx.obstacles, Some(cx.me as u64), end, path.end_dir, false)
                .min((CUT_TICKS - path.free).max(0.0) * onward);
            let mut mine = lines.clone();
            if let Some(leg) = mine.last_mut() {
                leg.to = end + path.end_dir * far;
                leg.tick_to = last + far / onward;
            }
            lines.extend(foe_lines.iter().copied());
            (
                i,
                race::run(cx.space, &mut scratch.race, &costs, &lines),
                mine,
            )
        })
        .collect();
    // The target is judged from the race of the plain ride, else the best line.
    let base = results
        .iter()
        .find(|(i, ..)| scored[*i].maneuver.kind == Kind::Hold)
        .or(results.first())?
        .1;
    let vulnerability = |foe: usize| {
        (1.0 + base[foe + 1] as f32).ln() + TARGET_DISTANCE * me.distance(cx.foes[foe].pos) / 1000.0
            - TARGET_RUBBER * (1.0 - cx.foes[foe].rubber / BASE_RUBBER).clamp(0.0, 1.0)
    };
    let best = (0..cx.foes.len()).min_by(|&a, &b| vulnerability(a).total_cmp(&vulnerability(b)))?;
    let kept = focus.and_then(|id| cx.foes.iter().position(|f| f.id == id));
    let target = match kept {
        Some(k) if vulnerability(k) <= vulnerability(best) + TARGET_STICK => k,
        _ => best,
    };
    let share = |cells: &Cells| {
        cells[0] as f32 / cells[..=foe_costs.len()].iter().sum::<u32>().max(1) as f32
    };
    let (base_share, base_held) = (share(&base).max(0.01), (base[target + 1] as f32).max(1.0));
    for (i, cells, mine) in &results {
        let line = &mut scored[*i];
        if !line.eligible {
            continue;
        }
        // Gains over riding straight; fighting only picks among safe lines.
        let gain = (share(cells) / base_share - 1.0).clamp(0.0, 1.0);
        let held = cells[target + 1] as f32;
        let squeeze = (1.0 - held / base_held).clamp(0.0, 1.0);
        let sealed = (1.0 - held / SEAL_CELLS).max(0.0);
        let safe = contact * line.safe;
        line.terms.control = safe * gain;
        let cut = cut_off(cx, mine, &cx.foes[target]);
        line.terms.pressure = safe * (squeeze + sealed + 2.0 * cut);
        line.score = cx.weights.value(&line.terms);
    }
    sort(scored);
    if me.distance(cx.foes[target].pos) <= REPLY_RANGE {
        for line in scored.iter_mut().filter(|l| l.eligible).take(REPLY_TOP) {
            let kill = sealing(cx, line, &cx.foes[target], scratch);
            line.terms.pressure += contact * line.safe * kill;
            line.score = cx.weights.value(&line.terms);
        }
        sort(scored);
    }
    Some(cx.foes[target].id)
}

/// How well the bot's lines stand across the straight line of `foe`: `0` when
/// they do not cross it, or the bot gets there too late or too close to the
/// foe's own arrival to be safe; up to `1` when they cross it just ahead of the
/// foe, so little that it has time to react. The sooner the better.
fn cut_off(cx: &Context, mine: &[Laid], foe: &Foe) -> f32 {
    let dir = cx.world.riders[foe.index].dir;
    let mut best = 0.0f32;
    for leg in mine {
        let Some(along) = ray_hit(foe.pos, dir, leg.from, leg.to) else {
            continue;
        };
        if along > foe.clear {
            continue;
        }
        let point = foe.pos + dir * along;
        let span = leg.from.distance(leg.to).max(1e-3);
        let ours = leg.tick_from + (leg.tick_to - leg.tick_from) * point.distance(leg.from) / span;
        let margin = along / foe.pace - ours;
        if margin < CUT_MIN_MARGIN {
            continue;
        }
        // Full value while the foe has little time to answer, fading as the
        // cut lies further ahead of it.
        let value = (1.0 - (margin - CUT_BEST_MARGIN) / CUT_FADE).clamp(0.0, 1.0);
        best = best.max(value);
    }
    best
}

/// How hard `line` leaves the target in the replies it has: nothing when it
/// keeps room, up to `1` when none of its replies lives through the horizon.
fn sealing(cx: &Context, line: &Scored, foe: &Foe, scratch: &mut Scratch) -> f32 {
    if line.path.blocked {
        return 0.0;
    }
    let rider = &cx.world.riders[foe.index];
    let points = &line.path.points[..line.path.len];
    // The line the bot would ride stands in the target's way.
    let mut obstacles = cx.obstacles.to_vec();
    obstacles.extend(Segment::trail_legs(points, cx.me as u64));
    let ride = Ride {
        obstacles: &obstacles,
        owner: foe.index,
        pos: rider.pos,
        dir: rider.dir,
        step: foe.pace,
        horizon: cx.params.horizon,
    };
    let mine: Vec<(Vec2, Vec2)> = points.windows(2).map(|pair| (pair[0], pair[1])).collect();
    let straight = Maneuver::hold();
    let replies = std::iter::once(straight).chain(
        REPLIES
            .iter()
            .map(|&(at, action)| Maneuver::new(Kind::Turn, &[Turn { at, action }])),
    );
    let mut best: Option<u32> = None;
    for reply in replies {
        let path = ride.trace(&reply);
        if path.blocked {
            continue;
        }
        let mut laid = path.legs();
        laid.extend_from_slice(&mine);
        let room = cx.space.measure(
            scratch,
            &laid,
            path.flood_start(),
            cx.params.horizon,
            cx.params.flood_cap,
        );
        best = Some(best.map_or(room.area, |b| b.max(room.area)));
    }
    match best {
        None => 1.0,
        Some(area) => 0.5 * (1.0 - area as f32 / NEED_CELLS).max(0.0),
    }
}

/// A path's legs with the tick each end is ridden.
fn laid_legs(path: &Path, step: f32) -> Vec<Laid> {
    let mut arc = 0.0;
    path.legs()
        .into_iter()
        .map(|(from, to)| {
            let tick_from = arc / step;
            arc += from.distance(to);
            Laid {
                owner: 0,
                from,
                to,
                tick_from,
                tick_to: arc / step,
            }
        })
        .collect()
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
    let radius = ticks as f32 * ROLLOUT_REACH + ROLLOUT_MARGIN;
    let mut rollout = Rollout::new(world, me, radius)?;
    (0..ticks).find(|&tick| !rollout.step(maneuver.action_at(tick)))
}

/// The maneuvers a bot with this budget weighs, in a fixed order: the plan it
/// is following, riding straight, a turn now or later, then reversals.
fn candidates(cx: &Context, current: Option<Maneuver>) -> Vec<Maneuver> {
    let params = cx.params;
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
    if params.variety >= 2 && cx.weights.grind > 0.0 {
        list.extend(lanes(cx));
    }
    if let Some((figure, progress)) = cx.figure {
        list.extend(figure.candidates(progress, cx.step, horizon));
    }
    list
}

/// Maneuvers aimed at the surroundings, to ride close beside something: a turn
/// timed to end up alongside the obstacle ahead, and for each side a sideways
/// step that ends up beside the trail or wall there.
fn lanes(cx: &Context) -> Vec<Maneuver> {
    let me = Some(cx.me as u64);
    let (pos, dir) = (cx.rider.pos, cx.rider.dir);
    let last = cx.params.horizon;
    let mut list = Vec::new();
    let front = nearest(cx.obstacles, me, pos, dir, false);
    for gap in LATE_GAPS {
        let at = ((front - gap) / cx.step).floor();
        if at >= 1.0 && (at as u32) < last {
            for action in [Action::Left, Action::Right] {
                list.push(Maneuver::new(
                    Kind::Turn,
                    &[Turn {
                        at: at as u32,
                        action,
                    }],
                ));
            }
        }
    }
    for (toward, back, side) in [
        (Action::Left, Action::Right, rotate_left(dir)),
        (Action::Right, Action::Left, rotate_right(dir)),
    ] {
        let beside = nearest(&cx.others, None, pos, side, true);
        if !(GRIND_RANGE..LANE_REACH).contains(&beside) {
            continue;
        }
        let at = ((beside - GRIND_GAP) / cx.step).floor() as u32;
        if at >= 1 && at + 1 < last {
            list.push(Maneuver::new(
                Kind::Dodge,
                &[
                    Turn {
                        at: 0,
                        action: toward,
                    },
                    Turn { at, action: back },
                ],
            ));
        }
    }
    list
}

/// Ride `maneuver` for the horizon along straight legs, stopping at the first
/// obstacle each leg meets.
fn trace(cx: &Context, maneuver: &Maneuver) -> Path {
    let ride = Ride {
        obstacles: cx.obstacles,
        owner: cx.me,
        pos: cx.rider.pos,
        dir: cx.rider.dir,
        step: cx.step,
        horizon: cx.params.horizon,
    };
    ride.trace(maneuver)
}

/// Where a rider starts a line, and what stands in its way.
struct Ride<'a> {
    obstacles: &'a [Segment],
    /// Index of the rider in the snapshot (the owner of its own trail).
    owner: usize,
    pos: Vec2,
    dir: Vec2,
    step: f32,
    horizon: u32,
}

impl Ride<'_> {
    fn trace(&self, maneuver: &Maneuver) -> Path {
        let horizon = self.horizon;
        let mut pos = self.pos;
        let mut dir = self.dir;
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
            let length = (until - tick) as f32 * self.step;
            if length > 0.0 {
                let hit = nearest(self.obstacles, Some(self.owner as u64), pos, dir, false);
                if hit < length {
                    pos += dir * hit;
                    path.free += hit / self.step;
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
}

/// How far a line's final heading points away from the nearest opponent,
/// `-1..=0` times the round's ramp, as soon as that opponent is out of
/// range. The heading is judged in coarse steps, so a foe on a diagonal does
/// not make the bot zigzag towards it. Without opponents it is nothing.
fn engage(cx: &Context, path: &Path) -> f32 {
    let Some(foe) = cx.foes.first() else {
        return 0.0;
    };
    let away = foe.pos - path.end();
    let far = ((away.length() - ENGAGE_RANGE) / ENGAGE_SPAN).clamp(0.0, 1.0);
    let cos = path.end_dir.dot(away.normalize_or_zero());
    let heading = (cos * ENGAGE_STEPS).floor() / ENGAGE_STEPS;
    // Faster than the opponent the bot hunts it down; slower, it hangs back.
    let pace = cx.rider.target_speed_mult
        / cx.world.riders[foe.index]
            .target_speed_mult
            .max(cx.world.riders[foe.index].speed_mult);
    -far * (1.0 - heading) * 0.5 * cx.ramp * pace.clamp(ENGAGE_PACE.0, ENGAGE_PACE.1)
}

/// Rubber a line burns against the obstacles ahead of its turns: a rider that
/// turns (or ends its line) inside the rubber zone of what it faces has been
/// slowed down on the way in, and pays for the distance it lost. A line that
/// runs into an obstacle burns everything.
fn rubber_burn(cx: &Context, path: &Path, speed: Speed) -> f32 {
    if path.blocked {
        return BASE_RUBBER;
    }
    let cruise = speed.mean.max(MIN_CRUISE);
    let (zone, normal) = (rubber_zone(cruise), normal_step(cruise));
    let mut burn = 0.0;
    for (from, to) in path.legs() {
        let dir = (to - from).normalize_or_zero();
        let ahead = nearest(cx.obstacles, Some(cx.me as u64), to, dir, false);
        if ahead >= zone {
            continue;
        }
        // Each tick in the zone closes `RUBBER_APPROACH` of the gap, so the
        // gap shrinks geometrically from the zone's edge down to `ahead`.
        let ticks = (zone / ahead.max(0.5)).ln() / -(1.0 - RUBBER_APPROACH).ln();
        let lost = ticks * normal - (zone - ahead);
        burn += lost.max(0.0) * RUBBER_COST * 2.0 / (1.0 + cruise);
    }
    burn
}

/// Measure and score one candidate.
fn evaluate(cx: &Context, index: usize, maneuver: &Maneuver, scratch: &mut Scratch) -> Scored {
    let params = cx.params;
    let path = trace(cx, maneuver);
    let end = path.end();
    // The opponents' lines ahead close room behind them as they ride.
    let mut laid = path.legs();
    laid.extend(cx.foes.iter().map(|foe| (foe.pos, foe.future)));
    let room = cx.space.measure(
        scratch,
        &laid,
        path.flood_start(),
        params.horizon,
        params.flood_cap,
    );
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
    let legs = path.legs();
    let speed = grind::along(
        &cx.others,
        &legs,
        cx.rider.target_speed_mult,
        cx.step,
        maneuver.first_action() != Action::Hold,
    );
    let cramped = 1.0 - (area_seen / NEED_CELLS).clamp(0.0, 1.0);
    let roomy = (area_seen / params.flood_cap as f32).clamp(0.0, 1.0);
    let safe = if path.blocked { 0.0 } else { roomy * roomy };
    let terms = Terms {
        space: ((1.0 + area_seen.max(0.0)).ln() / (1.0 + params.flood_cap as f32).ln())
            .clamp(0.0, 1.0),
        pocket: -cramped,
        reach,
        clear,
        turn: -(maneuver.turns().len() as f32) / MAX_TURNS as f32,
        fill: cramped * room.hug,
        commit: f32::from(
            maneuver.kind == Kind::Continue || (maneuver.kind == Kind::Hold && params.steady),
        ),
        pressure: 0.0,
        control: 0.0,
        engage: safe * engage(cx, &path),
        grind: safe * (speed.mean.min(GRIND_CAP) - cx.rider.target_speed_mult.min(GRIND_CAP))
            / (GRIND_CAP - 1.0),
        rubber: -(rubber_burn(cx, &path, speed) / (cx.rider.rubber + RUBBER_SLACK)).clamp(0.0, 1.0),
        style: cx.figure.map_or(0.0, |(figure, progress)| {
            safe * figure.rate(progress, maneuver, cx.step, params.horizon)
        }),
    };
    Scored {
        index,
        path,
        maneuver: *maneuver,
        // The fight terms join the score later, among the safe lines only.
        score: cx.weights.value(&Terms {
            engage: 0.0,
            ..terms
        }),
        terms,
        area: room.area,
        reach,
        safe,
        speed,
        eligible: true,
        bold: false,
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

    fn world_with(riders: Vec<RiderSnapshot>) -> WorldSnapshot {
        world(Vec::new(), riders)
    }

    fn params(level: u32, noise: f32) -> Params {
        let mut build = BotBuild::from_difficulty("t", Personality::Adaptive, Difficulty(level));
        build.knobs.noise = noise;
        Params::new(&build, Tier::Full, 1)
    }

    fn decide(world: &WorldSnapshot, params: &Params) -> Outcome {
        plan(world, 0, params, &mut Scratch::default(), None, None)
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

    /// A foe whose trail runs along the line `y = at` from `x0` to `x1`, its
    /// head at the east end, riding east.
    fn along_y(at: f32, x0: f32, x1: f32) -> RiderSnapshot {
        rider(
            Vec2::new(x1, at),
            Vec2::X,
            vec![Vec2::new(x0, at), Vec2::new(x1, at)],
        )
    }

    fn only(world: &WorldSnapshot, maneuver: Maneuver, scratch: &mut Scratch) -> Scored {
        let params = params(4000, 0.0);
        evaluate(&context(world, 0, &params), 0, &maneuver, scratch)
    }

    #[test]
    fn a_trail_to_the_side_gets_a_lane_aimed_beside_it() {
        let me = rider(Vec2::ZERO, Vec2::X, vec![Vec2::new(-30.0, 0.0)]);
        let world = world(Vec::new(), vec![me, along_y(150.0, -500.0, 500.0)]);
        let params = params(4000, 0.0);
        let lanes = lanes(&context(&world, 0, &params));
        // 150 units to the left, at 3 units a tick, less the gap kept.
        let step = lanes.iter().find(|m| {
            m.kind == Kind::Dodge && m.turns()[0].action == Action::Left && m.turns()[1].at == 46
        });
        assert!(step.is_some(), "{lanes:?}");
    }

    #[test]
    fn riding_beside_a_trail_earns_speed_and_open_space_does_not() {
        let me = rider(
            Vec2::new(-300.0, 8.0),
            Vec2::X,
            vec![Vec2::new(-330.0, 8.0)],
        );
        let world = world(Vec::new(), vec![me, along_y(0.0, -500.0, 500.0)]);
        let mut scratch = Scratch::default();
        let beside = only(&world, Maneuver::hold(), &mut scratch);
        let far = rider(
            Vec2::new(-300.0, 600.0),
            Vec2::X,
            vec![Vec2::new(-330.0, 600.0)],
        );
        let world = world_with(vec![far, along_y(0.0, -500.0, 500.0)]);
        let free = only(&world, Maneuver::hold(), &mut scratch);
        assert!(beside.speed.end > 1.2, "{:?}", beside.speed);
        assert!(beside.terms.grind > 0.0 && free.terms.grind <= 0.0);
    }

    #[test]
    fn a_late_turn_inside_the_rubber_zone_costs_rubber() {
        let me = rider(Vec2::new(950.0, 0.0), Vec2::X, vec![Vec2::new(900.0, 0.0)]);
        let world = world(Vec::new(), vec![me]);
        let turn = |at| {
            Maneuver::new(
                Kind::Turn,
                &[Turn {
                    at,
                    action: Action::Left,
                }],
            )
        };
        let mut scratch = Scratch::default();
        // 50 units to the wall: turning after 12 ticks leaves 14, after 15 ticks 5.
        let early = only(&world, turn(12), &mut scratch);
        let late = only(&world, turn(15), &mut scratch);
        assert_eq!(early.terms.rubber, 0.0);
        assert!(late.terms.rubber < 0.0, "{:?}", late.terms);
    }

    #[test]
    fn a_hot_round_trades_margin_for_the_fight() {
        let cold = Weights::BASE;
        assert_eq!(cold.heated(0.0), cold);
        // The fight comes first; margin and rubber's price go later.
        let warm = cold.heated(1.0);
        assert!(warm.pressure > cold.pressure && warm.risk > cold.risk);
        assert_eq!(warm.pocket, cold.pocket);
        let hot = cold.heated(3.0);
        assert!(hot.clear < cold.clear && hot.pocket < cold.pocket);
        assert!(hot.rubber < cold.rubber);
        // Only a bot told it plays a round heats up, and not at the start.
        let mut p = params(2000, 0.0);
        p.age = 10_000;
        assert_eq!(heat(&p), 0.0);
        p.rounds = true;
        assert!(heat(&p) > 1.0);
        p.age = 0;
        assert_eq!(heat(&p), 0.0);
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
