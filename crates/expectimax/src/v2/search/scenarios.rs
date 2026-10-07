//! Fixed stochastic scenarios allow candidate comparisons under the same outcomes.
use super::*;
/// The 11 dice totals with their exact probabilities out of 36.
pub fn dice_totals() -> [(u8, f64); 11] {
    std::array::from_fn(|i| {
        let total = i as u8 + 2;
        (total, f64::from(6 - total.abs_diff(7)) / 36.0)
    })
}
pub(super) fn dice_chance(total: u8) -> Chance {
    let first = total.saturating_sub(6).max(1);
    Chance::Dice {
        first,
        second: total - first,
    }
}
pub(super) fn stream(seed: u32, salt: u32) -> Random {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&seed.to_le_bytes());
    bytes[4..8].copy_from_slice(&salt.to_le_bytes());
    bytes[8..12].copy_from_slice(b"exp2");
    Random::new(bytes)
}
pub(super) fn pick_weighted(rng: &mut Random, weights: &[u32]) -> usize {
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return 0;
    }
    let mut n = rng.below(total);
    for (i, &w) in weights.iter().enumerate() {
        if n < w {
            return i;
        }
        n -= w;
    }
    weights.len() - 1
}
/// A common random scenario: stratified dice for the first rolls, then a private stream.
pub struct Scenario {
    rolls: Vec<u8>,
    pub(super) rng: Random,
    cursor: usize,
}
impl Scenario {
    pub fn set(count: usize, salt: u32, rolls_per_round: usize) -> Vec<Self> {
        let mut rng = stream(0xC0FFEE, salt);
        let mut perms: Vec<Vec<usize>> = (0..rolls_per_round)
            .map(|_| {
                let mut p: Vec<usize> = (0..count).collect();
                rng.shuffle(&mut p);
                p
            })
            .collect();
        let totals = dice_totals();
        (0..count)
            .map(|k| {
                let rolls = perms
                    .iter_mut()
                    .map(|perm| {
                        let u = (perm[k] as f64 + 0.5) / count as f64;
                        let mut acc = 0.0;
                        for &(total, p) in &totals {
                            acc += p;
                            if u < acc {
                                return total;
                            }
                        }
                        12
                    })
                    .collect();
                Self {
                    rolls,
                    rng: stream(k as u32 + 1, salt),
                    cursor: 0,
                }
            })
            .collect()
    }
    pub(super) fn roll(&mut self) -> u8 {
        let total = if self.cursor < self.rolls.len() {
            self.rolls[self.cursor]
        } else {
            (self.rng.below(6) + self.rng.below(6) + 2) as u8
        };
        self.cursor += 1;
        total
    }
    pub(super) fn reset(&mut self) {
        self.cursor = 0;
        // Reuse the same stratified rolls for every world and afterstate.
    }
}
