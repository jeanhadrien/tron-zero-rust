//! Display names for bots: drawn at random from a fixed list, never twice at
//! once. A name says nothing about the bot's strength or build.

use std::collections::HashSet;

use rand::seq::IndexedRandom;

const NAMES: &[&str] = &[
    "Ada", "Atlas", "Bishop", "Blitz", "Cinder", "Clu", "Cobalt", "Comet", "Cypher", "Dash",
    "Ember", "Echo", "Flux", "Gridley", "Halo", "Havoc", "Ion", "Jolt", "Kestrel", "Lumen", "Mako",
    "Mercury", "Nova", "Onyx", "Orbit", "Pixel", "Pulse", "Quill", "Rinzler", "Rogue", "Sark",
    "Spark", "Sable", "Tempest", "Torque", "Vector", "Volt", "Vortex", "Wisp", "Zephyr",
];

/// A random name not in `taken` (lowercase keys, so the match ignores case).
/// When the list is exhausted, falls back to a
/// numbered name (`Bot 7`), which is unique as long as `fallback_number` is.
pub(crate) fn pick_name(taken: &HashSet<String>, fallback_number: u32) -> String {
    let free: Vec<&str> = NAMES
        .iter()
        .copied()
        .filter(|name| !taken.contains(&name.to_lowercase()))
        .collect();
    free.choose(&mut rand::rng()).map_or_else(
        || format!("Bot {fallback_number}"),
        |name| (*name).to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_has_no_duplicates() {
        let unique: HashSet<_> = NAMES.iter().collect();
        assert_eq!(unique.len(), NAMES.len());
    }

    #[test]
    fn never_returns_a_taken_name() {
        let mut taken = HashSet::new();
        for number in 0..NAMES.len() as u32 + 5 {
            let name = pick_name(&taken, number);
            assert!(
                taken.insert(name.to_lowercase()),
                "duplicate name handed out"
            );
        }
    }
}
