//! Turn-level expectimax over sampled worlds.
//!
//! A level is one of the root player's turns. Within a level the search enumerates distinct
//! end-of-turn positions (afterstates) reachable by action sequences, with exact chance branches
//! for purchases, thefts, and the player's own roll. Between levels, opponents play a fixed
//! builder policy under common random scenarios: every afterstate and every root candidate sees
//! the same stratified dice, so comparisons are paired. Leaves are evaluated at the end of the
//! deepest planned turn. Iterative deepening keeps the deepest fully completed level count.
mod candidates;
mod opponents;
mod ordering;
mod planning;
mod scenarios;
use super::Leaf;
use super::RobberConfig;
use super::key::{Key, information_set};
use super::leaf::Terms;
use super::opponent;
use super::weights::Weights;
pub use ordering::{
    block_leader, needed_resource, road_preference, robber_preference, robber_preference_with,
    robber_rows,
};
#[cfg(test)]
use ordering::{road_base, road_score};
pub use scenarios::{Scenario, dice_totals};
use scenarios::{dice_chance, pick_weighted};
use serde::{Deserialize, Serialize};
use settlers_engine::*;
use std::collections::HashMap;

/// Afterstate groups of one line of play with their probability masses.
pub type Line = Vec<(f64, Vec<World>)>;
/// One end-of-turn position the search expects, with its absolute probability along the
/// current root action, its value, and the position itself (summarized before it leaves
/// the process; a report never carries hidden hands).
#[derive(Debug, Clone, Copy)]
pub struct Outcome {
    pub level: u8,
    pub probability: f64,
    pub value: f64,
    pub state: State,
}
type Recorded = Vec<(f64, f64, State)>;
/// Arrival groups of one opponents' round from an afterstate group: (mass share, worlds).
type Arrivals = Vec<(f64, Vec<World>)>;
/// A cached lookahead value: terms, recorded outcomes, search generation, transitions cost.
type MemoEntry = (Terms, Recorded, u32, u64);
/// Transposition tables shared by the depth passes of one decision, or by every decision of a
/// turn. Entries carry the search generation that made them and the transitions they cost, so
/// a hit from an earlier generation can report the work it saved. Values are identical with
/// and without the tables whenever no node or time budget binds; under a binding budget the
/// cached search completes more of the tree in the same budget.
#[derive(Default)]
pub struct Cache {
    pub lines: HashMap<Key, (f64, Line, u32, u64)>,
    /// Keyed by position, level, and the depth cap, since a value depends on how many more
    /// turns are planned below it.
    pub memo: HashMap<(Key, u8, u8), MemoEntry>,
    pub rounds: HashMap<(Key, u8), (Arrivals, u32, u64)>,
    /// The (turn, current seat) the entries belong to.
    pub owner: Option<(u32, u8)>,
    pub generation: u32,
}
impl Cache {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.memo.clear();
        self.rounds.clear();
        self.owner = None;
    }
    pub fn entries(&self) -> usize {
        self.lines.len() + self.memo.len() + self.rounds.len()
    }
}
#[derive(Debug, Clone, Copy)]
pub struct World {
    pub state: State,
    pub weight: f64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub transitions: u64,
    pub leaves: u64,
    pub afterstates: u64,
    pub expanded: u64,
    pub transpositions: u64,
    /// Hits on entries made by an earlier depth pass or decision, and the transitions they saved.
    pub cache_hits: u64,
    pub cached_transitions: u64,
    pub scenarios: u64,
    pub own_nodes: u64,
    pub per_level_nodes: Vec<u64>,
    pub per_level_afterstates: Vec<u64>,
    pub aborted: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub depth: u8,
    pub beam: usize,
    pub root_beam: usize,
    pub per_candidate: usize,
    pub afterstates: usize,
    pub scenarios: usize,
    pub inner_scenarios: usize,
    pub node_budget: u64,
    /// Opponents choose the action that maximizes their own static leaf instead of the builder's.
    pub leaf_opponents: bool,
    /// Opponents execute their predicted next build instead of following the builder.
    pub predict_opponents: bool,
    /// Setup candidates come from the opening planner's shortlist unless the search's own
    /// static-leaf shortlist is requested for ablation.
    pub opening: super::OpeningMode,
    /// Robber candidate scoring switches; see `RobberConfig`.
    pub robber: super::RobberConfig,
    /// Expected hidden-hand composition per seat from the analyzing seat's resource knowledge,
    /// used by the need-aware victim bonus. None disables it.
    pub expected_hands: Option<[[f64; 5]; 4]>,
    /// The bargaining `space` threat keys on predicted target sites instead of the best
    /// three sites; the robber threat arm uses the same definition.
    pub predict_threat: bool,
    /// Hand-leaf development term: value played knights by largest-army distance and credit a
    /// held playable knight with the production it would unblock.
    pub knight_play_value: bool,
    /// Offer a development purchase only when no build is affordable within the next own turn.
    pub buy_tempo: bool,
    /// Offer main-phase knight plays only when the robber sits on this seat's best hex, so the
    /// knight is held for the before-the-roll unblock.
    pub hold_knight: bool,
    /// Add bank trades and roads chosen only to shed cards when the hand is over the limit.
    pub spend_down: bool,
    /// Rank discard choices by the kept hand plus expected production before the next turn.
    pub discard_projection: bool,
}
pub struct Search<'a> {
    pub seat: u8,
    pub w: Weights,
    pub limits: Limits,
    pub stats: Stats,
    /// Returns true once the time budget is exhausted.
    pub over_time: &'a dyn Fn() -> bool,
    pub aborted: bool,
    /// The leaf evaluator: hand-written terms or learned tables.
    pub model: Leaf,
    /// Explain mode: record every expected end-of-turn position with its probability.
    pub record: bool,
    /// Probability mass of the path from the root action to the current planned turn.
    pub path_mass: f64,
    pub outcomes: Vec<Outcome>,
    cache: &'a mut Cache,
    generation: u32,
}
const MAX_STEPS: u32 = 400;

fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}
impl<'a> Search<'a> {
    pub fn new(
        seat: u8,
        w: Weights,
        limits: Limits,
        over_time: &'a dyn Fn() -> bool,
        model: Leaf,
        cache: &'a mut Cache,
    ) -> Self {
        cache.generation = cache.generation.wrapping_add(1);
        let generation = cache.generation;
        Self {
            seat,
            w,
            limits,
            stats: Stats::default(),
            over_time,
            aborted: false,
            model,
            record: false,
            path_mass: 1.0,
            outcomes: Vec::new(),
            cache,
            generation,
        }
    }
    /// Account for a hit on an entry made before this search generation: the work it saved.
    fn reuse(&mut self, generation: u32, ticks: u64) {
        if generation != self.generation {
            self.stats.cache_hits += 1;
            self.stats.cached_transitions += ticks;
        }
    }
    fn note_outcome(&mut self, level: u8, share: f64, value: f64, state: State) {
        if self.record {
            self.outcomes.push(Outcome {
                level,
                probability: self.path_mass * share,
                value,
                state,
            });
        }
    }
    /// Static value of a position from seat `p`'s perspective under the configured leaf.
    #[inline]
    pub fn value(&self, s: &State, p: u8) -> Terms {
        self.model
            .evaluate(s, p, &self.w, self.limits.knight_play_value)
    }
    fn tick(&mut self, n: u64) {
        self.stats.transitions += n;
        if self.stats.transitions > self.limits.node_budget
            || (self.stats.transitions % 128 < n && (self.over_time)())
        {
            self.aborted = true;
        }
    }
    pub fn leaf(&mut self, worlds: &[World]) -> Terms {
        let mut value = Terms::default();
        for w in worlds {
            let terms = self.value(&w.state, self.seat);
            value.add_scaled(&terms, w.weight);
            self.stats.leaves += 1;
        }
        value
    }
    fn mass(worlds: &[World]) -> f64 {
        worlds.iter().map(|w| w.weight).sum()
    }
    fn normalized(mut worlds: Vec<World>) -> Vec<World> {
        let mass = Self::mass(&worlds);
        if mass > 0.0 {
            for w in &mut worlds {
                w.weight /= mass;
            }
        }
        worlds
    }
    /// Split worlds by the root player's information set.
    /// Groups are ordered by first appearance, so the order of every sum over them, and with
    /// it the value bits, does not depend on the key function.
    fn regroup(&self, worlds: Vec<World>) -> Vec<(Key, Vec<World>)> {
        let mut index: HashMap<Key, usize> = HashMap::new();
        let mut groups: Vec<(Key, Vec<World>)> = Vec::new();
        for w in worlds {
            if w.weight > 1e-12 {
                let key = information_set(&w.state, self.seat);
                match index.get(&key) {
                    Some(&i) => groups[i].1.push(w),
                    None => {
                        index.insert(key, groups.len());
                        groups.push((key, vec![w]));
                    }
                }
            }
        }
        groups
    }
    /// Apply one action to every world with exact chance branches.
    pub fn step(&mut self, worlds: &[World], p: u8, a: Action) -> Result<Vec<World>, RuleError> {
        let mut out = Vec::with_capacity(worlds.len());
        for w in worlds {
            match a {
                Action::Roll => {
                    for (total, prob) in dice_totals() {
                        out.push(World {
                            state: apply_with_chance(&w.state, p, a, dice_chance(total))?,
                            weight: w.weight * prob,
                        });
                    }
                    self.tick(11);
                }
                Action::BuyDevelopment
                | Action::MoveRobber {
                    victim: Some(_), ..
                } => {
                    let branches = chance_outcomes(&w.state, p, a)?;
                    self.tick(branches.len() as u64);
                    for b in branches {
                        out.push(World {
                            state: apply_with_chance(&w.state, p, a, b.outcome)?,
                            weight: w.weight * f64::from(b.numerator) / f64::from(b.denominator),
                        });
                    }
                }
                _ => {
                    out.push(World {
                        state: apply(&w.state, p, a)?,
                        weight: w.weight,
                    });
                    self.tick(1);
                }
            }
        }
        Ok(out)
    }

    // ----- candidate generation -------------------------------------------------------------
}

#[cfg(test)]
#[path = "../../../tests/internal/v2_robber.rs"]
mod robber_tests;

#[cfg(test)]
#[path = "../../../tests/internal/road_block.rs"]
mod road_block_tests;

#[cfg(test)]
#[path = "../../../tests/internal/road_contention.rs"]
mod road_contention_tests;
