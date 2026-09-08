use rand_chacha::ChaCha12Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

/// Versioned counter-based randomness. The seed and cursor are private server state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Random {
    pub seed: [u8; 32],
    pub word: u64,
}
impl Random {
    pub fn new(seed: [u8; 32]) -> Self {
        Self { seed, word: 0 }
    }
    pub fn below(&mut self, bound: u32) -> u32 {
        assert!(bound > 0, "internal random bound must be positive");
        let mut rng = ChaCha12Rng::from_seed(self.seed);
        rng.set_word_pos(self.word as u128);
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let x = rng.next_u32();
            self.word += 1;
            if x >= threshold {
                return x % bound;
            }
        }
    }
    pub fn shuffle<T>(&mut self, values: &mut [T]) {
        for i in (1..values.len()).rev() {
            let j = self.below(i as u32 + 1) as usize;
            values.swap(i, j);
        }
    }
}
