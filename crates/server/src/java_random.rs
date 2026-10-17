//! Java's `Random` as vanilla's `LegacyRandomSource` uses it: a 48-bit
//! linear congruential generator. Where vanilla seeds it (the enchanting
//! table with the player's enchantment seed), the same seed gives the same
//! numbers, and so the same offers.

const MULTIPLIER: i64 = 0x5_DEEC_E66D;
const INCREMENT: i64 = 0xB;
const MASK: i64 = (1 << 48) - 1;

#[derive(Debug, Clone)]
pub struct JavaRandom {
    seed: i64,
}

impl JavaRandom {
    pub fn new(seed: i64) -> Self {
        let mut random = Self { seed: 0 };
        random.set_seed(seed);
        random
    }

    /// `setSeed`: scrambled with the multiplier.
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ MULTIPLIER) & MASK;
    }

    /// `next(bits)`.
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT)) & MASK;
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// `nextInt(bound)` as `BitRandomSource` does it.
    pub fn next_int_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        if bound.isolate_lowest_one() == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits - value + (bound - 1) >= 0 {
                return value;
            }
        }
    }

    /// `nextFloat`: 24 bits.
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * 5.960_464_5e-8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_numbers_as_java() {
        // As `java.util.Random` printed them.
        let mut random = JavaRandom::new(42);
        assert_eq!(random.next_int(), -1_170_105_035);
        assert_eq!(random.next_int_bounded(10), 3);
        let mut random = JavaRandom::new(0);
        assert_eq!(random.next_int_bounded(16), 11);
        let f = JavaRandom::new(1).next_float();
        assert!((f - 0.730_878_2).abs() < 1e-6, "{f}");
        let mut random = JavaRandom::new(-7);
        assert_eq!(
            [
                random.next_int_bounded(7),
                random.next_int_bounded(1000),
                random.next_int_bounded(8)
            ],
            [4, 297, 5]
        );
    }
}
