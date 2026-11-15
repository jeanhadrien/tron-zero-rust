//! Where a scene's riders begin: the scene decides, every runner applies it.
//! The runner gathers the `SpawnField` (walls of its world) and inserts
//! `shared::spawn::placed_at(&placement.start)` on its own entities.

use bots::Brain;
use shared::spawn::{SpawnField, Start};

use crate::Scene;

/// Spreads a rider's life counter over the seed space (golden-ratio odd
/// constant, as for the scene seed).
const LIFE_SEED_STEP: u64 = 0xA24B_AED4_963E_E407;
/// Spreads the scene seed over the seed space.
const SCENE_SEED_STEP: u64 = 0x9E37_79B9_7F4A_7C15;

/// One rider of a scene, ready to place.
#[derive(Clone, Debug)]
pub struct Placement {
    pub start: Start,
    /// The rider's own seed (`rider_seed`): later spawns add `lives` to it.
    pub seed: u64,
    /// Placements so far that used the seed: 1 after a safe spawn, 0 for a
    /// fixed start.
    pub lives: u64,
    /// A fresh brain for a bot rider, seeded from `seed` and `lives`.
    pub brain: Option<Brain>,
}

impl Scene {
    /// Place every rider, in file order, on an arena whose walls are in
    /// `field`. A fixed `at` is kept, and later spawns stay clear of it;
    /// the other riders take a safe spawn seeded by their name and `seed`.
    /// Bots get their brain, in planning slots by order among the bots.
    pub fn place(&self, field: &mut SpawnField, seed: u64) -> Result<Vec<Placement>, String> {
        let mut slot = 0;
        self.riders
            .iter()
            .map(|rider| {
                let rider_seed = rider_seed(&rider.name, seed);
                let (spawn, lives) = match rider.at {
                    Some(at) => {
                        field.occupy(at.position);
                        (at, 0)
                    }
                    None => (
                        field
                            .place(rider_seed)
                            .ok_or_else(|| format!("{}: no safe spawn", rider.name))?,
                        1,
                    ),
                };
                let brain = rider.bot.clone().map(|build| {
                    slot += 1;
                    Brain::new(build, brain_seed(rider_seed, lives), slot - 1)
                });
                Ok(Placement {
                    start: rider.start(spawn),
                    seed: rider_seed,
                    lives,
                    brain,
                })
            })
            .collect()
    }
}

/// A rider's seed: a stable hash of its name, plus the scene seed. Seed 0
/// keeps the plain name hash.
pub fn rider_seed(name: &str, seed: u64) -> u64 {
    fnv1a(name).wrapping_add(seed.wrapping_mul(SCENE_SEED_STEP))
}

/// Seed of a brain's misjudgments: the rider's seed, then its life counter.
pub fn brain_seed(rider_seed: u64, lives: u64) -> u64 {
    rider_seed ^ lives.wrapping_mul(LIFE_SEED_STEP)
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
