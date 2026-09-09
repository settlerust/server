use rand_chacha::ChaCha12Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

/// Versioned counter-based randomness. The seed and cursor are private server state.
///
/// In a deterministic game (`Settings::deterministic`) the shared cursor is not advanced
/// after setup; `word` instead packs the number of rolls taken so far (low 32 bits) and the
/// number of thefts so far (high 32 bits), and every outcome is read from a tape: a fixed
/// function of the seed, a domain, and the outcome's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Random {
    pub seed: [u8; 32],
    pub word: u64,
}
impl Random {
    pub const TAPE_DICE: u32 = 1;
    pub const TAPE_STEAL: u32 = 2;
    pub const TAPE_DECK: u32 = 3;
    /// Position of a tape entry. Domains sit above 2^40 words, far beyond any live cursor.
    fn tape_position(domain: u32, index: u32) -> u128 {
        (u128::from(domain) << 40) | (u128::from(index) << 4)
    }
    /// Uniform value below `bound` for one tape entry, independent of every other entry.
    pub fn tape(seed: [u8; 32], domain: u32, index: u32, bound: u32) -> u32 {
        assert!(bound > 0, "internal random bound must be positive");
        let mut rng = ChaCha12Rng::from_seed(seed);
        rng.set_word_pos(Self::tape_position(domain, index));
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let x = rng.next_u32();
            if x >= threshold {
                return x % bound;
            }
        }
    }
    /// A fixed shuffle for a whole domain, for example the development deck order.
    pub fn tape_shuffle<T>(seed: [u8; 32], domain: u32, values: &mut [T]) {
        let mut rng = ChaCha12Rng::from_seed(seed);
        rng.set_word_pos(Self::tape_position(domain, 0));
        for i in (1..values.len()).rev() {
            let bound = i as u32 + 1;
            let threshold = bound.wrapping_neg() % bound;
            let j = loop {
                let x = rng.next_u32();
                if x >= threshold {
                    break x % bound;
                }
            };
            values.swap(i, j as usize);
        }
    }
    /// Deterministic dice: the next roll index and both dice from the dice tape.
    pub fn tape_roll(&mut self) -> (u8, u8) {
        let index = (self.word & 0xffff_ffff) as u32;
        self.word += 1;
        (
            Self::tape(self.seed, Self::TAPE_DICE, index * 2, 6) as u8 + 1,
            Self::tape(self.seed, Self::TAPE_DICE, index * 2 + 1, 6) as u8 + 1,
        )
    }
    /// Deterministic theft: a uniform position below `bound` from the theft tape.
    pub fn tape_steal(&mut self, bound: u32) -> u32 {
        let index = (self.word >> 32) as u32;
        self.word += 1 << 32;
        Self::tape(self.seed, Self::TAPE_STEAL, index, bound)
    }
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
