//! The match: one or more rounds played by the configured `MatchRules`.
//!
//! Every server runs one ([`MatchState`], from the config's `[match]` and
//! `[round]`). The state machine is `shared::MatchFlow`; the system
//! [`advance_match`] applies its decisions to the world: it retires and places
//! riders, keeps the scores and publishes the replicated [`MatchStatus`].
//! Respawns ask [`MatchState::may_spawn`]. Nothing here is read by
//! `simulate_players`. Normative rules: `docs/specs/match-rules.md`.

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::spawn::{SpawnField, placed};
use shared::{
    DeathLog, DisplayName, IsAlive, IsColliding, IsSliding, LifeGeneration, MatchFlow, MatchRules,
    MatchStatus, MatchStep, Player, Roster, Rubber, Score, ShouldHandleDeath, SpeedMult,
    TargetSpeedMult, Trail, round_winner,
};

use crate::systems::{Human, InitialSpawn};

/// The match as the server runs it: the shared flow, plus what only the server
/// keeps.
#[derive(Resource, Debug)]
pub struct MatchState {
    flow: MatchFlow,
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
            flow: MatchFlow::new(rules),
            winner: None,
            begun: 0,
            status: None,
        }
    }

    /// Already playing round 1 (unit tests of the respawn paths).
    #[cfg(test)]
    pub(crate) fn in_round(rules: MatchRules) -> Self {
        Self {
            flow: MatchFlow::in_round(rules),
            ..Self::new(rules)
        }
    }

    pub fn rules(&self) -> &MatchRules {
        self.flow.rules()
    }

    /// Whether a dead rider may be placed now (see `MatchFlow::may_spawn`).
    pub fn may_spawn(&self, human: bool, riders: &Riders) -> bool {
        self.flow.may_spawn(human, &riders.roster())
    }

    /// The replicated view of the match.
    fn status(&self, riders: &Riders) -> MatchStatus {
        MatchStatus {
            rounds: self.flow.rules().rounds,
            round: self.flow.round(),
            phase: self.flow.phase(),
            secs_left: self.flow.secs_left(),
            winner: self.winner.clone(),
            respawn_now: self.may_spawn(true, riders),
        }
    }
}

/// Every rider in the arena, sorted so placement is stable: what the flow
/// sees of the world.
#[derive(Debug)]
pub struct Riders {
    /// (rider, alive, human).
    all: Vec<(Entity, bool, bool)>,
    alive: Vec<Entity>,
}

impl Riders {
    pub fn of(world: &mut World) -> Self {
        let mut all: Vec<(Entity, bool, bool)> = world
            .query_filtered::<(Entity, &IsAlive, Has<Human>), With<Player>>()
            .iter(world)
            .map(|(entity, alive, human)| (entity, alive.0, human))
            .collect();
        all.sort_by_key(|(entity, ..)| *entity);
        let alive = all
            .iter()
            .filter(|(_, alive, _)| *alive)
            .map(|(entity, ..)| *entity)
            .collect();
        Self { all, alive }
    }

    fn roster(&self) -> Roster<'_> {
        let humans = |alive_only: bool| {
            self.all
                .iter()
                .filter(|(_, alive, human)| *human && (*alive || !alive_only))
                .count()
        };
        Roster {
            riders: self.all.len(),
            humans: humans(false),
            alive: &self.alive,
            humans_alive: humans(true),
        }
    }
}

/// Credit this tick's deaths to the scores. Runs after `simulate_players` and
/// before `resolve_deaths`, which drains the log.
pub fn count_scores(deaths: Res<DeathLog>, mut scores: Query<&mut Score, With<Player>>) {
    for (entity, credit) in Score::credits(&deaths.0) {
        if let Ok(mut score) = scores.get_mut(entity) {
            score.add(credit);
        }
    }
}

/// Run the match one tick, apply what it decided and publish the status.
/// Runs after the tick's simulation and death resolution. Exclusive: placing
/// riders needs every rider's fresh pose.
pub fn advance_match(world: &mut World) {
    let riders = Riders::of(world);
    let step = world
        .resource_mut::<MatchState>()
        .flow
        .advance(&riders.roster());
    match step {
        MatchStep::Idle => {}
        MatchStep::Starting => {
            info!(riders = riders.all.len(), "Match: starting");
            retire(world, &riders.alive);
        }
        MatchStep::RoundOver { reason, winner } => {
            let rules = *world.resource::<MatchState>().rules();
            let winner = round_winner(&rules, reason, winner, || scores(world));
            let name = winner.and_then(|entity| {
                world.get_mut::<Score>(entity)?.wins += 1;
                Some(world.get::<DisplayName>(entity)?.0.clone())
            });
            info!(winner = ?name, ?reason, "Match: round over");
            world.resource_mut::<MatchState>().winner = name;
            retire(world, &riders.alive);
        }
        MatchStep::Begin { new_match } => begin_round(world, &riders, new_match),
    }
    if !world.resource::<MatchState>().flow.is_running() {
        // Waiting: no winner to show, and only humans ride.
        world.resource_mut::<MatchState>().winner = None;
        let bots: Vec<Entity> = riders
            .all
            .iter()
            .filter(|(_, alive, human)| *alive && !*human)
            .map(|(entity, ..)| *entity)
            .collect();
        retire(world, &bots);
    }
    // The step changes who is alive, not who is here: the roster still says
    // whether a human may respawn.
    publish_status(world, &riders);
}

fn scores(world: &mut World) -> Vec<(Entity, Score)> {
    world
        .query_filtered::<(Entity, &Score), With<Player>>()
        .iter(world)
        .map(|(entity, score)| (entity, *score))
        .collect()
}

/// Stop riders where they are: dead, trail gone, as after a crash. Used on the
/// round's survivors, who would otherwise keep riding through the break (and
/// could die in it, which would reach the ranked reports).
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
/// gone: the dead lost theirs and the survivors were retired), humans in a new
/// life so their previous round's commands are stale.
fn begin_round(world: &mut World, riders: &Riders, new_match: bool) {
    let (round, begun) = {
        let mut state = world.resource_mut::<MatchState>();
        state.begun += 1;
        if new_match {
            state.winner = None;
        }
        (state.flow.round(), state.begun)
    };
    let Some(mut field) = SpawnField::walls(world) else {
        warn!("Match: no arena, nobody can be placed");
        return;
    };
    info!(round, riders = riders.all.len(), "Match: round starts");

    for &(entity, ..) in &riders.all {
        let mut rider = world.entity_mut(entity);
        rider.remove::<InitialSpawn>();
        if new_match && let Some(mut score) = rider.get_mut::<Score>() {
            *score = Score::default();
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
        match field.place(seed) {
            Some(spawn) => {
                rider.insert(placed(&spawn));
                // A bot starts the round with a fresh brain: no plan, no
                // target, and its engagement clock back at zero.
                if let Some(mut bot) = rider.get_mut::<crate::bot::BotBrain>() {
                    bot.brain = bot.brain.reborn_for_round(rand::random());
                }
            }
            None => warn!(
                ?entity,
                "Match: no safe spawn, the rider sits this round out"
            ),
        }
    }
}

/// Mirror the match into the replicated status entity.
fn publish_status(world: &mut World, riders: &Riders) {
    let status = world.resource::<MatchState>().status(riders);
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
    use shared::{EndRules, MatchPhase, Respawn};

    fn rules(rounds: u32, limit: Option<u32>) -> MatchRules {
        MatchRules {
            respawn: Respawn::Never,
            end: EndRules {
                time_limit: limit,
                last_standing: true,
                no_human_left: true,
            },
            rounds,
            intermission: 3,
            final_intermission: 5,
            needs_human: true,
        }
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
                let mut rider =
                    world.spawn((Player, Score::default(), DisplayName(format!("r{i}"))));
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

    fn alive(world: &World, rider: Entity) -> bool {
        world.get::<IsAlive>(rider).unwrap().0
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
            assert!(alive(&world, rider));
        }
        assert_eq!(world.get::<LifeGeneration>(riders[0]).unwrap().0, 8);
        let heads: Vec<Vec2> = riders
            .iter()
            .map(|&r| world.get::<shared::Position>(r).unwrap().0)
            .collect();
        assert!(heads[0].distance(heads[1]) >= shared::spawn::SPAWN_CLEARANCE);

        // r1 and r2 die: r0 wins, is retired, and its score says so.
        kill(&mut world, riders[1]);
        kill(&mut world, riders[2]);
        advance_match(&mut world);
        let now = status(&mut world);
        assert_eq!(now.phase, MatchPhase::Intermission);
        assert_eq!(now.winner.as_deref(), Some("r0"));
        assert_eq!(world.get::<Score>(riders[0]).unwrap().wins, 1);
        assert!(!alive(&world, riders[0]));

        // After the break the second round places everybody again.
        for _ in 0..3 {
            advance_match(&mut world);
        }
        let now = status(&mut world);
        assert_eq!((now.phase, now.round), (MatchPhase::Playing, 2));
        assert!(alive(&world, riders[1]));
        assert_eq!(world.get::<LifeGeneration>(riders[0]).unwrap().0, 9);
    }

    #[test]
    fn a_rider_joining_mid_round_waits_and_is_placed_with_the_next_round() {
        let (mut world, riders) = world_with(rules(3, None), 3);
        for _ in 0..4 {
            advance_match(&mut world);
        }
        let late = world
            .spawn((
                Player,
                Human,
                Score::default(),
                LifeGeneration(1),
                InitialSpawn,
                DisplayName("late".into()),
            ))
            .id();
        advance_match(&mut world);
        // Dead and counted for nothing: the round goes on.
        assert!(!alive(&world, late));
        assert_eq!(status(&mut world).phase, MatchPhase::Playing);
        kill(&mut world, riders[1]);
        kill(&mut world, riders[2]);
        for _ in 0..4 {
            advance_match(&mut world);
        }
        assert!(alive(&world, late));
        assert!(!world.entity(late).contains::<InitialSpawn>());
    }

    #[test]
    fn the_round_ends_when_the_last_human_is_out_and_bots_wait_without_humans() {
        let (mut world, riders) = world_with(rules(3, None), 3);
        for _ in 0..4 {
            advance_match(&mut world);
        }
        // The human dies; two bots still ride: the round is over at once.
        kill(&mut world, riders[0]);
        advance_match(&mut world);
        let now = status(&mut world);
        assert_eq!(now.phase, MatchPhase::Intermission);
        assert_eq!(now.winner, None);

        // The human leaves: the match is dropped and the bots stay off.
        world.despawn(riders[0]);
        for _ in 0..10 {
            advance_match(&mut world);
        }
        assert_eq!(status(&mut world).phase, MatchPhase::Free);
        assert!(!alive(&world, riders[1]) && !alive(&world, riders[2]));
    }

    #[test]
    fn a_round_out_of_time_goes_to_the_top_scorer() {
        let rules = MatchRules {
            respawn: Respawn::After(10),
            end: EndRules {
                time_limit: Some(5),
                ..EndRules::default()
            },
            ..rules(1, None)
        };
        let (mut world, riders) = world_with(rules, 2);
        for _ in 0..4 {
            advance_match(&mut world);
        }
        world.get_mut::<Score>(riders[1]).unwrap().kills = 2;
        for _ in 0..5 {
            advance_match(&mut world);
        }
        let now = status(&mut world);
        assert_eq!(now.phase, MatchPhase::MatchOver);
        assert_eq!(now.winner.as_deref(), Some("r1"));
        assert!(!now.respawn_now, "nobody respawns on the end screen");
    }
}
