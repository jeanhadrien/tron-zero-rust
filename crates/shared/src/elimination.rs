//! Rated elimination events, resolved from the sim's raw death log.
//!
//! Generic over the rider id so the server (bot build ids, human ids) and the
//! lab (bot build ids, scenario names) share one resolution.

use bevy_ecs::prelude::Entity;
use serde::Serialize;

use crate::{DeathCause, DeathRecord};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Elimination<Id> {
    /// `victim` died on `killer`'s trail.
    Kill {
        tick: Option<u32>,
        victim: Id,
        killer: Id,
    },
    /// Both died on each other's trail on the same tick.
    Mutual { tick: Option<u32>, a: Id, b: Id },
    /// `victim` hit a wall or its own trail; `alive` are the riders still
    /// alive at the end of the tick.
    Suicide {
        tick: Option<u32>,
        victim: Id,
        alive: Vec<Id>,
    },
}

/// Pair same-tick mutual kills and report every other death. Deaths of riders
/// `rated_id` does not know are skipped.
pub fn resolve_eliminations<Id: Clone>(
    records: &[DeathRecord],
    rated_id: impl Fn(Entity) -> Option<Id>,
    alive: &[Id],
) -> Vec<Elimination<Id>> {
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

    fn id(entity: Entity) -> Option<String> {
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
        let out = resolve_eliminations(&records, id, &[]);
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
        let out = resolve_eliminations(&records, id, &["r3".to_string()]);
        assert!(
            matches!(&out[0], Elimination::Kill { victim, killer, .. } if victim == "r1" && killer == "r2")
        );
        assert!(
            matches!(&out[1], Elimination::Suicide { alive, .. } if alive == &["r3".to_string()])
        );
    }
}
