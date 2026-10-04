//! A tiny random number generator so flair does not need a dependency for wiggles.

use std::time::{SystemTime, UNIX_EPOCH};

/// A xorshift generator. Not for anything that matters, just for sparks and critters.
#[derive(Debug, Clone)]
pub struct Rng {
    /// The current state, never zero.
    state: u64,
}

impl Default for Rng {
    fn default() -> Self {
        Self::seeded(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0x9e37_79b9, |time| {
                    u64::try_from(time.as_nanos() & u128::from(u64::MAX)).unwrap_or(1)
                }),
        )
    }
}

impl Rng {
    /// Creates a generator from `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    /// Returns the next raw number.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Returns a number in `0..n`, or 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % u64::try_from(n).unwrap_or(u64::MAX)).unwrap_or(0)
    }

    /// Returns a float in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        // 24 bits fit a f32 mantissa exactly
        let bits = u32::try_from(self.next_u64() >> 40).unwrap_or(0);
        bits as f32 / (1u32 << 24) as f32
    }

    /// Returns a float in `lo..hi`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Returns a random element of `items`.
    ///
    /// # Panics
    ///
    /// Panics if `items` is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

#[cfg(test)]
/// Tests for [`Rng`].
mod tests {
    use super::Rng;

    /// Numbers stay in their ranges.
    #[test]
    fn stays_in_range() {
        let mut rng = Rng::seeded(42);
        for _ in 0..1000 {
            assert!(rng.below(7) < 7);
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit));
        }
        assert_eq!(rng.below(0), 0);
    }
}
