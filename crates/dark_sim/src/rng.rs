//! A small seeded random generator that is part of the saved world, so a run replays exactly.

use serde::{Deserialize, Serialize};

/// SplitMix64: fast, good enough for dice, identical on every platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`; `n` must be positive.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        // Multiply-shift keeps the bias far below anything a game could notice.
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: u128, den: u128) -> bool {
        if den == 0 || num >= den {
            return den != 0;
        }
        // Scale both to 64 bits so huge powers still compare fairly.
        let shift = 128 - den.leading_zeros();
        let shift = shift.saturating_sub(64);
        let (num, den) = ((num >> shift) as u64, ((den >> shift) as u64).max(1));
        self.below(den) < num
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.below(u64::from(hi.saturating_sub(lo)) + 1) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_numbers() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn chance_matches_its_odds() {
        let mut rng = Rng::new(1);
        let hits = (0..10_000).filter(|_| rng.chance(1, 4)).count();
        assert!((2_300..2_700).contains(&hits), "{hits}");
        assert!(!rng.chance(0, 5));
        assert!(rng.chance(5, 5));
        assert!(!rng.chance(1, 0));
        // Huge values keep their ratio.
        let hits = (0..10_000)
            .filter(|_| rng.chance(3u128 << 100, 4u128 << 100))
            .count();
        assert!((7_300..7_700).contains(&hits), "{hits}");
    }

    #[test]
    fn range_is_inclusive() {
        let mut rng = Rng::new(3);
        let seen: std::collections::BTreeSet<u32> = (0..200).map(|_| rng.range(2, 4)).collect();
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![2, 3, 4]);
    }
}
