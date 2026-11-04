//! Turns the sim's raw death log into rated elimination events.
//!
//! Rated ids are resolved at the death tick, before any despawn: bots use
//! their build identity, humans a placeholder derived from `PlayerId` until
//! accounts exist. Nothing here talks to the network; a later reporter drains
//! [`Eliminations`].

use bevy::prelude::*;
use shared::{DeathCause, DeathLog, DeathRecord, IsAlive, Player, PlayerId};

use crate::bot::BotBrain;

/// Identity the rating pool knows a rider by (`bot:<personality>:<skill>`, or a
/// human id).
pub type RatedId = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Elimination {
    /// `victim` died on `killer`'s trail.
    Kill {
        tick: Option<u32>,
        victim: RatedId,
        killer: RatedId,
    },
    /// Both died on each other's trail on the same tick.
    Mutual {
        tick: Option<u32>,
        a: RatedId,
        b: RatedId,
    },
    /// `victim` hit a wall or its own trail; `alive` are the riders still
    /// alive at the end of the tick.
    Suicide {
        tick: Option<u32>,
        victim: RatedId,
        alive: Vec<RatedId>,
    },
}

/// Resolved eliminations awaiting a consumer (the future manager reporter).
#[derive(Resource, Default, Debug)]
pub struct Eliminations(pub Vec<Elimination>);

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
    out.0.extend(resolve(&records, rated_id, &alive));
    // No consumer exists yet: keep the newest events only, never grow forever.
    let excess = out.0.len().saturating_sub(MAX_PENDING);
    out.0.drain(..excess);
}

/// Upper bound on unconsumed eliminations kept in memory.
const MAX_PENDING: usize = 1024;

/// Pure resolution: pair same-tick mutual kills, report the rest.
fn resolve(
    records: &[DeathRecord],
    rated_id: impl Fn(Entity) -> Option<RatedId>,
    alive: &[RatedId],
) -> Vec<Elimination> {
    let killed_by = |victim: Entity| {
        records.iter().find_map(|r| match r.cause {
            DeathCause::Trail(killer) if r.victim == victim => Some(killer),
            _ => None,
        })
    };
    let mut out = Vec::new();
    for record in records {
        let Some(victim) = rated_id(record.victim) else {
            continue;
        };
        match record.cause {
            DeathCause::Wall | DeathCause::OwnTrail => out.push(Elimination::Suicide {
                tick: record.tick,
                victim,
                alive: alive.to_vec(),
            }),
            DeathCause::Trail(killer) => {
                let Some(killer_id) = rated_id(killer) else {
                    continue;
                };
                if killed_by(killer) == Some(record.victim) {
                    // Report a mutual pair once, from its lower entity.
                    if record.victim < killer {
                        out.push(Elimination::Mutual {
                            tick: record.tick,
                            a: victim,
                            b: killer_id,
                        });
                    }
                } else {
                    out.push(Elimination::Kill {
                        tick: record.tick,
                        victim,
                        killer: killer_id,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(victim: Entity, cause: DeathCause) -> DeathRecord {
        DeathRecord {
            tick: Some(10),
            victim,
            cause,
        }
    }

    fn id(entity: Entity) -> Option<RatedId> {
        Some(format!("r{}", entity.index()))
    }

    #[test]
    fn same_tick_head_on_is_one_mutual_kill() {
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        let records = [
            record(a, DeathCause::Trail(b)),
            record(b, DeathCause::Trail(a)),
        ];
        let out = resolve(&records, id, &[]);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Elimination::Mutual { .. }));
    }

    #[test]
    fn one_sided_death_is_a_kill_and_a_wall_is_a_suicide() {
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        let records = [record(a, DeathCause::Trail(b)), record(b, DeathCause::Wall)];
        let out = resolve(&records, id, &["r3".into()]);
        assert!(
            matches!(&out[0], Elimination::Kill { victim, killer, .. } if victim == "r1" && killer == "r2")
        );
        assert!(
            matches!(&out[1], Elimination::Suicide { alive, .. } if alive == &["r3".to_string()])
        );
    }
}
