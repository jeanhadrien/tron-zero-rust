//! Seeded misjudgment. A weak bot reads distances and room wrongly; it never
//! acts at random. The draws are pure functions of the brain's seed, the
//! decision tick and what is being judged, so a replay misjudges identically.

/// A draw in `[-1, 1]` for `(tick, candidate, lane)`. `lane` tells apart the
/// quantities judged for one candidate.
pub fn unit(seed: u64, tick: u32, candidate: usize, lane: u8) -> f32 {
    let mut z = seed
        .wrapping_add(u64::from(tick).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(
            ((candidate as u64) << 8 | u64::from(lane)).wrapping_mul(0xD1B5_4A32_D192_ED03),
        );
    // splitmix64 finalizer: neighbouring inputs give unrelated outputs.
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let fraction = (z >> 40) as f32 / (1u64 << 24) as f32;
    fraction * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_reproducible_and_spread_over_the_range() {
        assert_eq!(unit(7, 100, 3, 1), unit(7, 100, 3, 1));
        assert_ne!(unit(7, 100, 3, 1), unit(8, 100, 3, 1));
        for seed in [0, 7] {
            let draws: Vec<f32> = (0..2000).map(|tick| unit(seed, tick, 0, 0)).collect();
            assert!(draws.iter().all(|d| (-1.0..=1.0).contains(d)));
            let low = draws.iter().filter(|&&d| d < -0.8).count();
            assert!((140..260).contains(&low), "seed {seed}: {low}");
        }
    }
}
