//! A small, fast PRNG (splitmix64), so seeding is reproducible without a crate.

pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in 0..n (n > 0). The modulo bias is negligible for small n.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let draw = |seed| {
            let mut rng = SplitMix64(seed);
            [rng.next_u64(), rng.next_u64(), rng.next_u64()]
        };
        assert_eq!(draw(42), draw(42));
        assert_ne!(draw(42), draw(43));
        // The reference splitmix64 output for seed 0.
        assert_eq!(SplitMix64(0).next_u64(), 0xe220_a839_7b1d_cdaf);
    }

    #[test]
    fn ranges() {
        let mut rng = SplitMix64(7);
        for _ in 0..1000 {
            let x = rng.unit();
            assert!((0.0..1.0).contains(&x));
            assert!(rng.below(3) < 3);
        }
        assert_eq!(rng.below(1), 0);
    }
}
