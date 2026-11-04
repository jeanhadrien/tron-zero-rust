//! Turns the sim's raw death log into rated elimination events.
//!
//! Rated ids are resolved at the death tick, before any despawn: bots use
//! their build identity, humans their verified account (`acct:<id>`). A human
//! without an account (an unranked server) has no rated id and is skipped.
//! Nothing here talks to the network; the ranked announcer drains
//! [`Eliminations`].

use bevy::prelude::*;
use shared::{DeathLog, Elimination, IsAlive, Player, resolve_eliminations};

use crate::bot::BotBrain;
use crate::ranked::AccountId;

/// Identity the rating pool knows a rider by (`bot:<personality>:<skill>`, or a
/// human id).
pub type RatedId = String;

/// Resolved eliminations awaiting a consumer (the future manager reporter).
#[derive(Resource, Default, Debug)]
pub struct Eliminations(pub Vec<Elimination<RatedId>>);

/// Upper bound on unconsumed eliminations kept in memory.
const MAX_PENDING: usize = 1024;

type RiderIdentities<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static AccountId>,
        Option<&'static BotBrain>,
        &'static IsAlive,
    ),
    With<Player>,
>;

/// The rating identity of a rider: its bot build, or its verified account.
fn rated_id_of(account: Option<&AccountId>, bot: Option<&BotBrain>) -> Option<RatedId> {
    match (bot, account) {
        (Some(bot), _) => Some(bot.brain.build.rating_id()),
        (None, Some(account)) => Some(format!("acct:{}", account.id)),
        (None, None) => None,
    }
}

/// Drain the sim's `DeathLog` into [`Eliminations`]. Runs right after
/// `simulate_players`, so the alive set is the end-of-tick state and nobody
/// has been despawned yet.
pub fn resolve_deaths(
    mut deaths: ResMut<DeathLog>,
    mut out: ResMut<Eliminations>,
    riders: RiderIdentities,
) {
    if deaths.0.is_empty() {
        return;
    }
    let rated_id = |entity: Entity| -> Option<RatedId> {
        let (account, bot, _) = riders.get(entity).ok()?;
        rated_id_of(account, bot)
    };
    let alive: Vec<RatedId> = riders
        .iter()
        .filter(|(.., alive)| alive.0)
        .filter_map(|(account, bot, _)| rated_id_of(account, bot))
        .collect();

    let records = std::mem::take(&mut deaths.0);
    out.0
        .extend(resolve_eliminations(&records, rated_id, &alive));
    // No consumer exists yet: keep the newest events only, never grow forever.
    let excess = out.0.len().saturating_sub(MAX_PENDING);
    out.0.drain(..excess);
}
