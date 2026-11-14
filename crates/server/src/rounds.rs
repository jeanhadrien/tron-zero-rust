//! Rounds mode: last rider standing per round, over a fixed number of rounds.
//!
//! The server has this mode when [`MatchState`] exists (`GAME_MODE=rounds`,
//! see [`rules_from_env`]); without it the game is the free-for-all. The state
//! machine ([`MatchState::advance`]) is pure and counts fixed ticks; the
//! system [`advance_match`] applies its decisions to the world: it retires and
//! places riders and publishes the replicated [`MatchStatus`]. Nothing here is
//! read by `simulate_players`. Normative rules: `docs/specs/rounds-mode.md`.

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::match_rules::secs_to_ticks;
use shared::spawn::find_safe_spawn;
use shared::{
    DisplayName, EndRules, IsAlive, IsColliding, IsSliding, LifeGeneration, MatchPhase, MatchRules,
    MatchStatus, Player, Respawn, RoundEnd, Rubber, ShouldHandleDeath, SpeedMult, TICK_HZ,
    TargetSpeedMult, Trail,
};

use crate::systems::{Human, InitialSpawn};

/// Riders (humans and bots, alive or waiting) a match needs to run.
const MIN_RIDERS: usize = 2;

const DEFAULT_ROUNDS: u32 = 5;
const DEFAULT_ROUND_SECS: u32 = 90;
const DEFAULT_INTERMISSION_SECS: u32 = 3;
const DEFAULT_FINAL_INTERMISSION_SECS: u32 = 8;

/// Round wins of a rider, this match. Server-only; clients get the names and
/// totals through [`MatchStatus::standings`].
#[derive(Component, Default, Debug, PartialEq, Eq)]
pub struct Wins(pub u32);

/// What the controller decided for one tick; the world applies it.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    /// A second rider is here: stop everybody, count down to round 1.
    Starting,
    /// Place every rider and start a round; `new_match` resets the scores.
    Begin {
        new_match: bool,
    },
    /// The round is over: `winner` is the last rider alive, if there is one.
    RoundOver {
        winner: Option<Entity>,
    },
}

/// The match: rules, phase and clock. Present only in rounds mode.
#[derive(Resource, Debug)]
pub struct MatchState {
    rules: MatchRules,
    phase: MatchPhase,
    /// Current round from 1; 0 while `Free`.
    round: u32,
    /// Ticks spent in this phase.
    ticks: u32,
    /// Name of the last round's winner.
    winner: Option<String>,
    /// Placements so far; varies the spawn seeds from round to round.
    begun: u64,
    /// The replicated status entity, spawned on first use.
    status: Option<Entity>,
}

impl MatchState {
    pub fn new(rules: MatchRules) -> Self {
        Self {
            rules,
            phase: MatchPhase::Free,
            round: 0,
            ticks: 0,
            winner: None,
            begun: 0,
            status: None,
        }
    }

    pub fn phase(&self) -> MatchPhase {
        self.phase
    }

    /// Whether the controller runs the riders. When false the server behaves as
    /// free-for-all: free spawns and manual respawns.
    pub fn is_running(&self) -> bool {
        self.phase != MatchPhase::Free
    }

    /// Advance one fixed tick. `riders` counts everybody in the arena roster,
    /// `alive` those still riding.
    pub fn advance(&mut self, riders: usize, alive: &[Entity]) -> Step {
        match self.phase {
            MatchPhase::Free => {
                if riders < MIN_RIDERS {
                    return Step::Idle;
                }
                self.enter(MatchPhase::Starting, 1);
                Step::Starting
            }
            MatchPhase::Starting => {
                if !self.wait(riders, self.rules.intermission) {
                    return Step::Idle;
                }
                self.enter(MatchPhase::Playing, 1);
                Step::Begin { new_match: true }
            }
            MatchPhase::Playing => {
                self.ticks += 1;
                let Some(end) = self.rules.round_end(self.ticks, alive.len()) else {
                    return Step::Idle;
                };
                let winner = match (end, alive) {
                    (RoundEnd::LastStanding, &[only]) => Some(only),
                    _ => None,
                };
                let phase = if self.round >= self.rules.rounds {
                    MatchPhase::MatchOver
                } else {
                    MatchPhase::Intermission
                };
                self.enter(phase, self.round);
                Step::RoundOver { winner }
            }
            MatchPhase::Intermission => {
                if !self.wait(riders, self.rules.intermission) {
                    return Step::Idle;
                }
                self.enter(MatchPhase::Playing, self.round + 1);
                Step::Begin { new_match: false }
            }
            MatchPhase::MatchOver => {
                if !self.wait(riders, self.rules.final_intermission) {
                    return Step::Idle;
                }
                self.enter(MatchPhase::Playing, 1);
                Step::Begin { new_match: true }
            }
        }
    }

    fn enter(&mut self, phase: MatchPhase, round: u32) {
        self.phase = phase;
        self.round = round;
        self.ticks = 0;
    }

    /// Count a break tick; true once it has lasted `len` ticks. With too few
    /// riders left the match is dropped and the server frees up.
    fn wait(&mut self, riders: usize, len: u32) -> bool {
        if riders < MIN_RIDERS {
            self.enter(MatchPhase::Free, 0);
            self.winner = None;
            return false;
        }
        self.ticks += 1;
        self.ticks >= len
    }

    /// Ticks the current phase's clock lasts, `None` without a limit.
    fn clock_len(&self) -> Option<u32> {
        match self.phase {
            MatchPhase::Free => None,
            MatchPhase::Playing => self.rules.end.time_limit,
            MatchPhase::Starting | MatchPhase::Intermission => Some(self.rules.intermission),
            MatchPhase::MatchOver => Some(self.rules.final_intermission),
        }
    }

    /// The replicated view of the match, without the standings.
    fn status(&self) -> MatchStatus {
        let secs_left = self.clock_len().map(|len| {
            let left = len.saturating_sub(self.ticks).div_ceil(TICK_HZ as u32);
            left.min(u32::from(u16::MAX)) as u16
        });
        MatchStatus {
            rounds: self.rules.rounds,
            round: self.round,
            phase: self.phase,
            secs_left,
            winner: self.winner.clone(),
            standings: Vec::new(),
        }
    }
}

/// Read the game mode from the environment: `GAME_MODE` is `ffa` (default,
/// `None`) or `rounds`, tuned by `ROUNDS`, `ROUND_SECS` (0: no time limit),
/// `INTERMISSION_SECS` and `FINAL_INTERMISSION_SECS`. An invalid value is an
/// error: the server must not silently run another mode than asked.
pub fn rules_from_env(get: &dyn Fn(&str) -> Option<String>) -> Result<Option<MatchRules>, String> {
    match get("GAME_MODE").as_deref().map(str::trim) {
        None | Some("") | Some("ffa") => return Ok(None),
        Some("rounds") => {}
        Some(other) => return Err(format!("GAME_MODE must be ffa or rounds, got {other:?}")),
    }
    let number = |key: &str, default: u32| match get(key).as_deref().map(str::trim) {
        None | Some("") => Ok(default),
        Some(raw) => raw
            .parse::<u32>()
            .map_err(|_| format!("{key} must be a whole number, got {raw:?}")),
    };
    let round_secs = number("ROUND_SECS", DEFAULT_ROUND_SECS)?;
    let rules = MatchRules {
        respawn: Respawn::Never,
        end: EndRules {
            time_limit: (round_secs > 0).then(|| secs_to_ticks(round_secs)),
            last_standing: true,
        },
        rounds: number("ROUNDS", DEFAULT_ROUNDS)?,
        intermission: secs_to_ticks(number("INTERMISSION_SECS", DEFAULT_INTERMISSION_SECS)?),
        final_intermission: secs_to_ticks(number(
            "FINAL_INTERMISSION_SECS",
            DEFAULT_FINAL_INTERMISSION_SECS,
        )?),
    };
    rules.validate()?;
    if rules.intermission == 0 || rules.final_intermission == 0 {
        return Err("intermissions must last at least 1 second".into());
    }
    Ok(Some(rules))
}

/// Run the match one tick, apply what it decided and publish the status.
/// Runs after the tick's simulation and death resolution. Exclusive: placing
/// riders needs every rider's fresh pose.
pub fn advance_match(world: &mut World) {
    // Every rider gets a score; the roster is sorted so placement is stable.
    let unscored: Vec<Entity> = world
        .query_filtered::<Entity, (With<Player>, Without<Wins>)>()
        .iter(world)
        .collect();
    for entity in unscored {
        world.entity_mut(entity).insert(Wins::default());
    }
    let mut roster: Vec<(Entity, bool)> = world
        .query_filtered::<(Entity, &IsAlive), With<Player>>()
        .iter(world)
        .map(|(entity, alive)| (entity, alive.0))
        .collect();
    roster.sort_by_key(|(entity, _)| *entity);
    let alive: Vec<Entity> = roster
        .iter()
        .filter(|(_, alive)| *alive)
        .map(|(entity, _)| *entity)
        .collect();

    let step = world
        .resource_mut::<MatchState>()
        .advance(roster.len(), &alive);
    match step {
        Step::Idle => {}
        Step::Starting => {
            info!(riders = roster.len(), "Rounds: a match is starting");
            retire(world, &alive);
        }
        Step::RoundOver { winner } => {
            let name = winner.and_then(|entity| {
                world.get_mut::<Wins>(entity)?.0 += 1;
                Some(world.get::<DisplayName>(entity)?.0.clone())
            });
            info!(winner = ?name, "Rounds: round over");
            world.resource_mut::<MatchState>().winner = name;
            retire(world, &alive);
        }
        Step::Begin { new_match } => begin_round(world, &roster, new_match),
    }
    publish_status(world);
}

/// Stop riders where they are: dead, trail gone, as after a crash. Used on the
/// round's survivor, who would otherwise keep riding through the intermission
/// (and could die in it, which would reach the ranked reports).
fn retire(world: &mut World, riders: &[Entity]) {
    for &entity in riders {
        world.entity_mut(entity).insert((
            IsAlive(false),
            ShouldHandleDeath(false),
            SpeedMult(0.0),
            TargetSpeedMult(0.0),
            Rubber(0.0),
            IsSliding(false),
            IsColliding(false),
            Trail::default(),
        ));
    }
}

/// Place every rider, one after the other, on a clear arena (all trails are
/// gone: the dead lost theirs and the survivor was retired), humans in a new
/// life so their previous round's commands are stale.
fn begin_round(world: &mut World, roster: &[(Entity, bool)], new_match: bool) {
    let (round, begun) = {
        let mut state = world.resource_mut::<MatchState>();
        state.begun += 1;
        if new_match {
            state.winner = None;
        }
        (state.round, state.begun)
    };
    let mut arenas =
        world.query_filtered::<(&shared::ArenaSize, &shared::WallSegments), With<shared::Arena>>();
    let Ok((size, walls)) = arenas.single(world) else {
        warn!("Rounds: no arena, nobody can be placed");
        return;
    };
    let size = *size;
    let segments: Vec<[Vec2; 2]> = walls
        .0
        .iter()
        .map(|w| [Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])])
        .collect();
    info!(round, riders = roster.len(), "Rounds: round starts");

    let mut placed: Vec<Vec2> = Vec::new();
    for &(entity, _) in roster {
        let mut rider = world.entity_mut(entity);
        rider.remove::<InitialSpawn>();
        if new_match && let Some(mut wins) = rider.get_mut::<Wins>() {
            wins.0 = 0;
        }
        let life = rider
            .get::<LifeGeneration>()
            .and_then(|generation| generation.next());
        if rider.contains::<Human>()
            && let Some(life) = life
        {
            rider.insert(life);
        }
        let seed = entity.to_bits().wrapping_add(begun);
        match find_safe_spawn(size, &segments, &placed, seed) {
            Some(spawn) => {
                placed.push(spawn.position);
                rider.insert(crate::systems::placed(&spawn));
            }
            None => warn!(
                ?entity,
                "Rounds: no safe spawn, the rider sits this round out"
            ),
        }
    }
}

/// Mirror the match into the replicated status entity, standings included.
fn publish_status(world: &mut World) {
    let mut status = world.resource::<MatchState>().status();
    let mut scores: Vec<(String, u32)> = world
        .query_filtered::<(&DisplayName, &Wins), With<Player>>()
        .iter(world)
        .map(|(name, wins)| (name.0.clone(), wins.0))
        .collect();
    scores.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    status.standings = scores;

    let existing = world.resource::<MatchState>().status;
    match existing.and_then(|entity| world.get_mut::<MatchStatus>(entity)) {
        Some(mut current) => {
            current.set_if_neq(status);
        }
        None => {
            let entity = world
                .spawn((status, Replicate::to_clients(NetworkTarget::All)))
                .id();
            world.resource_mut::<MatchState>().status = Some(entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(rounds: u32, limit: Option<u32>) -> MatchRules {
        MatchRules {
            respawn: Respawn::Never,
            end: EndRules {
                time_limit: limit,
                last_standing: true,
            },
            rounds,
            intermission: 3,
            final_intermission: 5,
        }
    }

    fn entities(n: u32) -> Vec<Entity> {
        (1..=n)
            .map(Entity::from_raw_u32)
            .map(Option::unwrap)
            .collect()
    }

    /// Run `ticks` idle ticks with everyone alive, then return the last step.
    fn idle(state: &mut MatchState, riders: &[Entity], ticks: u32) {
        for _ in 0..ticks {
            assert_eq!(state.advance(riders.len(), riders), Step::Idle);
        }
    }

    /// Bring a fresh match to the first tick of round 1.
    fn playing(state: &mut MatchState, riders: &[Entity]) {
        assert_eq!(state.advance(riders.len(), riders), Step::Starting);
        idle(state, riders, 2);
        assert_eq!(
            state.advance(riders.len(), riders),
            Step::Begin { new_match: true }
        );
        assert_eq!(state.phase(), MatchPhase::Playing);
    }

    #[test]
    fn fewer_than_two_riders_keep_the_server_free() {
        let mut state = MatchState::new(rules(2, None));
        let riders = entities(1);
        idle(&mut state, &riders, 10);
        assert!(!state.is_running());
        assert_eq!(state.advance(0, &[]), Step::Idle);
    }

    #[test]
    fn a_second_rider_starts_a_countdown_then_round_one() {
        let mut state = MatchState::new(rules(2, None));
        let riders = entities(2);
        playing(&mut state, &riders);
        assert!(state.is_running());
        assert_eq!(state.round, 1);
    }

    #[test]
    fn the_last_rider_alive_wins_and_the_break_precedes_the_next_round() {
        let mut state = MatchState::new(rules(3, None));
        let riders = entities(3);
        playing(&mut state, &riders);
        idle(&mut state, &riders, 5);
        // Two of three die.
        assert_eq!(
            state.advance(3, &riders[..1]),
            Step::RoundOver {
                winner: Some(riders[0])
            }
        );
        assert_eq!(state.phase(), MatchPhase::Intermission);
        assert_eq!(state.round, 1);
        // The break lasts `intermission` ticks, then round 2 begins.
        idle(&mut state, &riders, 2);
        assert_eq!(state.advance(3, &riders), Step::Begin { new_match: false });
        assert_eq!((state.phase(), state.round), (MatchPhase::Playing, 2));
    }

    #[test]
    fn nobody_wins_when_the_last_riders_die_together_or_time_runs_out() {
        let mut state = MatchState::new(rules(3, Some(10)));
        let riders = entities(3);
        playing(&mut state, &riders);
        assert_eq!(state.advance(3, &[]), Step::RoundOver { winner: None });
        idle(&mut state, &riders, 2);
        assert_eq!(state.advance(3, &riders), Step::Begin { new_match: false });
        // Time limit with several alive: round 2 lasts exactly 10 ticks.
        idle(&mut state, &riders, 9);
        assert_eq!(state.advance(3, &riders), Step::RoundOver { winner: None });
    }

    #[test]
    fn the_last_round_leads_to_the_standings_then_a_new_match() {
        let mut state = MatchState::new(rules(1, None));
        let riders = entities(2);
        playing(&mut state, &riders);
        assert_eq!(
            state.advance(2, &riders[1..]),
            Step::RoundOver {
                winner: Some(riders[1])
            }
        );
        assert_eq!(state.phase(), MatchPhase::MatchOver);
        idle(&mut state, &riders, 4);
        assert_eq!(state.advance(2, &riders), Step::Begin { new_match: true });
        assert_eq!((state.phase(), state.round), (MatchPhase::Playing, 1));
    }

    #[test]
    fn a_leaver_counts_as_eliminated_and_too_few_riders_end_the_match() {
        let mut state = MatchState::new(rules(3, None));
        let riders = entities(2);
        playing(&mut state, &riders);
        // One rider left the arena: one alive, the survivor wins.
        assert_eq!(
            state.advance(1, &riders[..1]),
            Step::RoundOver {
                winner: Some(riders[0])
            }
        );
        // Still alone after the break started: back to free riding.
        assert_eq!(state.advance(1, &riders[..1]), Step::Idle);
        assert!(!state.is_running());
        assert_eq!(state.round, 0);
    }

    #[test]
    fn the_clock_counts_down_in_whole_seconds() {
        let mut state = MatchState::new(rules(2, Some(secs_to_ticks(90))));
        assert_eq!(state.status().secs_left, None);
        let riders = entities(2);
        state.advance(2, &riders);
        assert_eq!(state.status().secs_left, Some(1));
        state.enter(MatchPhase::Free, 0);
        playing(&mut state, &riders);
        assert_eq!(state.status().secs_left, Some(90));
        idle(&mut state, &riders, secs_to_ticks(1));
        assert_eq!(state.status().secs_left, Some(89));
    }

    #[test]
    fn mode_comes_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(rules_from_env(&env(&[])), Ok(None));
        assert_eq!(rules_from_env(&env(&[("GAME_MODE", "ffa")])), Ok(None));
        let rules = rules_from_env(&env(&[("GAME_MODE", "rounds")]))
            .unwrap()
            .unwrap();
        assert_eq!(rules.rounds, 5);
        assert_eq!(rules.end.time_limit, Some(secs_to_ticks(90)));
        assert_eq!(rules.intermission, secs_to_ticks(3));
        let custom = rules_from_env(&env(&[
            ("GAME_MODE", "rounds"),
            ("ROUNDS", "7"),
            ("ROUND_SECS", "0"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!((custom.rounds, custom.end.time_limit), (7, None));
        assert!(rules_from_env(&env(&[("GAME_MODE", "tdm")])).is_err());
        assert!(rules_from_env(&env(&[("GAME_MODE", "rounds"), ("ROUNDS", "0")])).is_err());
        assert!(rules_from_env(&env(&[("GAME_MODE", "rounds"), ("ROUNDS", "x")])).is_err());
    }

    /// A world with an arena and `n` riders (the first is a human), match
    /// state in `rules`.
    fn world_with(rules: MatchRules, n: usize) -> (World, Vec<Entity>) {
        let mut world = World::new();
        world.insert_resource(MatchState::new(rules));
        world.spawn((
            shared::Arena,
            shared::ArenaSize::default(),
            shared::WallSegments(shared::arena_walls(
                shared::ARENA_WIDTH,
                shared::ARENA_HEIGHT,
            )),
        ));
        let riders = (0..n)
            .map(|i| {
                let mut rider = world.spawn((Player, DisplayName(format!("r{i}"))));
                if i == 0 {
                    rider.insert((Human, LifeGeneration(7)));
                }
                rider.id()
            })
            .collect();
        (world, riders)
    }

    fn kill(world: &mut World, rider: Entity) {
        world.entity_mut(rider).insert(IsAlive(false));
    }

    fn status(world: &mut World) -> MatchStatus {
        world
            .query::<&MatchStatus>()
            .single(world)
            .expect("one status")
            .clone()
    }

    #[test]
    fn the_world_follows_a_round_from_placement_to_the_next_one() {
        let (mut world, riders) = world_with(rules(2, None), 3);
        // Countdown, then everyone is placed alive and the human is in a new life.
        for _ in 0..4 {
            advance_match(&mut world);
        }
        assert_eq!(status(&mut world).phase, MatchPhase::Playing);
        for &rider in &riders {
            assert!(world.get::<IsAlive>(rider).unwrap().0);
        }
        assert_eq!(world.get::<LifeGeneration>(riders[0]).unwrap().0, 8);
        let heads: Vec<Vec2> = riders
            .iter()
            .map(|&r| world.get::<shared::Position>(r).unwrap().0)
            .collect();
        assert!(heads[0].distance(heads[1]) >= shared::spawn::SPAWN_CLEARANCE);

        // r1 and r2 die: r0 wins, is retired, and the standings say so.
        kill(&mut world, riders[1]);
        kill(&mut world, riders[2]);
        advance_match(&mut world);
        let now = status(&mut world);
        assert_eq!(now.phase, MatchPhase::Intermission);
        assert_eq!(now.winner.as_deref(), Some("r0"));
        assert_eq!(now.standings[0], ("r0".to_owned(), 1));
        assert!(!world.get::<IsAlive>(riders[0]).unwrap().0);

        // After the break the second round places everybody again.
        for _ in 0..3 {
            advance_match(&mut world);
        }
        let now = status(&mut world);
        assert_eq!((now.phase, now.round), (MatchPhase::Playing, 2));
        assert!(world.get::<IsAlive>(riders[1]).unwrap().0);
        assert_eq!(world.get::<LifeGeneration>(riders[0]).unwrap().0, 9);
    }

    #[test]
    fn a_rider_joining_mid_round_waits_and_is_placed_with_the_next_round() {
        let (mut world, riders) = world_with(rules(3, None), 2);
        for _ in 0..4 {
            advance_match(&mut world);
        }
        let late = world
            .spawn((
                Player,
                Human,
                LifeGeneration(1),
                InitialSpawn,
                DisplayName("late".into()),
            ))
            .id();
        advance_match(&mut world);
        // Dead and counted for nothing: the round goes on with two alive.
        assert!(!world.get::<IsAlive>(late).unwrap().0);
        assert_eq!(status(&mut world).phase, MatchPhase::Playing);
        kill(&mut world, riders[1]);
        for _ in 0..4 {
            advance_match(&mut world);
        }
        assert!(world.get::<IsAlive>(late).unwrap().0);
        assert!(!world.entity(late).contains::<InitialSpawn>());
        assert_eq!(status(&mut world).standings.len(), 3);
    }
}
