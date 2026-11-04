//! Turns the sim's raw death log into rated elimination events.
//!
//! Rated ids are resolved at the death tick, before any despawn: bots use
//! their build identity, humans a placeholder derived from `PlayerId` until
//! accounts exist. Nothing here talks to the network; a later reporter drains
//! [`Eliminations`].

use bevy::prelude::*;
use shared::{DeathLog, Elimination, IsAlive, Player, PlayerId, resolve_eliminations};

use crate::bot::BotBrain;

/// Identity the rating pool knows a rider by (`bot:<personality>:<skill>`, or a
/// human id).
pub type RatedId = String;

/// Resolved eliminations awaiting a consumer (the future manager reporter).
#[derive(Resource, Default, Debug)]
pub struct Eliminations(pub Vec<Elimination<RatedId>>);

/// Upper bound on unconsumed eliminations kept in memory.
const MAX_PENDING: usize = 1024;

/// Drain the sim's `DeathLog` into [`Eliminations`]. Runs right after
/// `simulate_players`, so the alive set is the end-of-tick state and nobody
/// has been despawned yet.
pub fn resolve_deaths(
    mut deaths: ResMut<DeathLog>,
    mut out: ResMut<Eliminations>,
    riders: Query<(Entity, &PlayerId, Option<&BotBrain>, &IsAlive), With<Player>>,
) {
    if deaths.0.is_empty() {
        return;
    }
    let rated_id = |entity: Entity| -> Option<RatedId> {
        let (_, id, bot, _) = riders.get(entity).ok()?;
        Some(match bot {
            Some(bot) => bot.brain.build.rating_id(),
            None => id.0.clone(),
        })
    };
    let alive: Vec<RatedId> = riders
        .iter()
        .filter(|(_, _, _, alive)| alive.0)
        .filter_map(|(entity, ..)| rated_id(entity))
        .collect();

    let records = std::mem::take(&mut deaths.0);
    out.0
        .extend(resolve_eliminations(&records, rated_id, &alive));
    // No consumer exists yet: keep the newest events only, never grow forever.
    let excess = out.0.len().saturating_sub(MAX_PENDING);
    out.0.drain(..excess);
}
