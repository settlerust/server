//! Turn-level expectimax over sampled worlds.
//!
//! A level is one of the root player's turns. Within a level the search enumerates distinct
//! end-of-turn positions (afterstates) reachable by action sequences, with exact chance branches
//! for purchases, thefts, and the player's own roll. Between levels, opponents play a fixed
//! builder policy under common random scenarios: every afterstate and every root candidate sees
//! the same stratified dice, so comparisons are paired. Leaves are evaluated at the end of the
//! deepest planned turn. Iterative deepening keeps the deepest fully completed level count.
use super::Leaf;
use super::RobberConfig;
use super::key::{Key, information_set};
use super::leaf::Terms;
use super::opponent;
use super::weights::Weights;
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
/// The points leader to slow down, when the late-game endgame switch is armed: past
/// `block_from` of the estimated game length, the opponent with the most points.
pub fn block_leader(s: &State, p: u8, w: &Weights) -> Option<u8> {
    if !w.endgame.block_leader {
        return None;
    }
    let length = w.endgame.block_from * w.turns_per_player * f64::from(s.settings.players);
    if f64::from(s.turn) < length {
        return None;
    }
    super::leaf::leader_seat(s, p, w)
}
/// Roads worth considering for seat `p`: those that improve its own reach, and with the
/// late endgame switch on, those that lengthen the leader's route to the best site they
/// contest with this seat.
/// Shared measurements for scoring candidate roads: the seat's current reach, the trail
/// race, the contested site a blocking road could take, and the leading opponent's
/// reachable value when the `opponent_expansion` term is on.
struct RoadBase {
    base_best: f64,
    contend: bool,
    own_length: u8,
    deny: Option<(u8, u8)>,
    their_route: Option<[u8; 54]>,
    base_opp: f64,
}
fn road_base(s: &State, p: u8, w: &Weights) -> RoadBase {
    let own = s.players[p as usize];
    let (base_best, _) = super::leaf::reach(s, p);
    let leader = (0..s.settings.players)
        .filter(|&o| o != p)
        .map(|o| road_length(s, o))
        .max()
        .unwrap_or(0)
        .max(4);
    let own_length = road_length(s, p);
    let contend = own.roads >= 4
        && own_length + 2 >= leader
        && s.longest_road != Some(p)
        && (w.road_contention_gate == 0.0 || super::leaf::road_production_lead(s, p));
    let deny =
        block_leader(s, p, w).and_then(|q| super::bargain::contested_site(s, p, q).map(|v| (q, v)));
    let their_route = deny.map(|(q, _)| super::bargain::distances(s, q));
    let base_opp = if w.opponent_expansion != 0.0 {
        super::leaf::opponent_reach(s, p)
    } else {
        0.0
    };
    RoadBase {
        base_best,
        contend,
        own_length,
        deny,
        their_route,
        base_opp,
    }
}
/// Candidate score of one road: the gain in own reachable-site value and site count, a
/// bonus for extending a contested trail, the endgame credit for cutting the leader's
/// route to the contested site, and, with the `opponent_expansion` term on, the denied
/// value of the leading opponent's reachable sites. The trail bonus asks whether the seat
/// may contend for the award: four or more roads within two of the leader, and, with the
/// `road_contention_gate` weight on, the table's best brick and lumber production.
fn road_score(s: &State, next: &State, p: u8, w: &Weights, base: &RoadBase) -> f64 {
    let (best, others) = super::leaf::reach(next, p);
    let mut score = best - base.base_best + f64::from(others) * 0.02;
    if base.contend && road_length(next, p) > base.own_length {
        score += 0.5;
    }
    if let (Some((q, v)), Some(route)) = (base.deny, &base.their_route) {
        let theirs = route[v as usize];
        let after = super::bargain::distances(next, q)[v as usize];
        let denied = f64::from(after.saturating_sub(theirs)).min(3.0);
        if theirs < u8::MAX && denied > 0.0 {
            score += w.endgame.block_bias * crate::leaf::site_value(s, v)
                / (1.0 + f64::from(theirs))
                * denied
                / 3.0
                * 0.5;
        }
    }
    if w.opponent_expansion != 0.0 {
        score += w.opponent_expansion * (base.base_opp - super::leaf::opponent_reach(next, p));
    }
    score
}
pub fn road_preference(s: &State, p: u8, w: &Weights, cap: usize) -> Vec<Action> {
    let own = s.players[p as usize];
    if own.roads >= 15
        || !s.players[p as usize].resources.contains(Resources::ROAD) && s.phase == Phase::Main
    {
        return vec![];
    }
    let base = road_base(s, p, w);
    let mut scored: Vec<(f64, u8)> = Vec::new();
    for e in 0..72u8 {
        if !road_allowed(s, p, e) {
            continue;
        }
        let Ok(next) = apply(s, p, Action::BuildRoad { edge: e }) else {
            continue;
        };
        let score = road_score(s, &next, p, w, &base);
        if score > 1e-9 {
            scored.push((score, e));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(cap)
        .map(|(_, e)| Action::BuildRoad { edge: e })
        .collect()
}
/// Robber placements worth considering for seat `p` under the default scoring, with the
/// late endgame switch adding its leader bias when armed.
pub fn robber_preference(s: &State, p: u8, w: &Weights, cap: usize) -> Vec<Action> {
    robber_preference_with(s, p, w, cap, RobberConfig::default(), None, false)
}
/// The same shortlist under the robber targeting switches; see `RobberConfig`.
pub fn robber_preference_with(
    s: &State,
    p: u8,
    w: &Weights,
    cap: usize,
    robber: RobberConfig,
    expected_hands: Option<[[f64; 5]; 4]>,
    predict_threat: bool,
) -> Vec<Action> {
    let rows = robber_rows(s, p, w, robber, expected_hands, predict_threat);
    if rows.is_empty() {
        return vec![opponent::robber(s, p)];
    }
    let mut rows = rows;
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    rows.into_iter().take(cap).map(|(_, a)| a).collect()
}
/// Every legal robber placement with its static score, before the shortlist cut: the
/// production it blocks and the cards it steals, plus the endgame leader bias and the
/// robber targeting switches when armed. The score ranks the shortlist; it is not the
/// value the lookahead assigns.
pub fn robber_rows(
    s: &State,
    p: u8,
    w: &Weights,
    robber: RobberConfig,
    expected_hands: Option<[[f64; 5]; 4]>,
    predict_threat: bool,
) -> Vec<(f64, Action)> {
    let mut scored: Vec<(f64, Action)> = Vec::new();
    let endgame_leader = block_leader(s, p, w);
    let threats = robber
        .threat
        .then(|| super::bargain::threats(s, p, predict_threat));
    let target_leader = robber
        .leader
        .then(|| {
            (0..s.settings.players)
                .filter(|&o| o != p)
                .max_by_key(|&o| (public_points(s, o), std::cmp::Reverse(o)))
        })
        .flatten();
    let needed = if robber.need {
        needed_resource(s, p, w)
    } else {
        None
    };
    for hex in 0..19u8 {
        if hex == s.robber {
            continue;
        }
        let pips = f64::from(6u8.saturating_sub(s.layout.number(hex).abs_diff(7)));
        let mut blocked = 0.0;
        let mut leader_blocked = 0.0;
        let mut own = false;
        for &v in &topology().hex_vertices[hex as usize] {
            if let Some(o) = s.board.owner(v) {
                let scale = if s.board.building(v) > 4 { 2.0 } else { 1.0 };
                if o == p {
                    own = true;
                    continue;
                }
                let mut weight = 1.0 + f64::from(public_points(s, o)) * 0.15;
                if Some(o) == endgame_leader {
                    weight += w.endgame.block_bias * super::leaf::opponent_points(s, o, w) * 0.15;
                }
                let mut term = pips * scale * weight;
                if let Some(threats) = &threats {
                    term *= 1.0 + 2.0 * threats[o as usize].total;
                }
                blocked += term;
                if Some(o) == target_leader {
                    leader_blocked += term;
                }
            }
        }
        if own || s.layout.resource(hex).is_none() {
            continue;
        }
        let hex_score = if target_leader.is_some() {
            leader_blocked + 0.1 * (blocked - leader_blocked)
        } else {
            blocked
        };
        let victims = robber_victims(s, p, hex);
        if victims.is_empty() {
            scored.push((hex_score, Action::MoveRobber { hex, victim: None }));
        }
        for v in victims {
            let cards = f64::from(s.players[v as usize].resources.total()).min(8.0);
            let hand_term = match (needed, expected_hands) {
                (Some(r), Some(table)) => {
                    let expected = table[v as usize];
                    let total = expected.iter().sum::<f64>().max(1.0);
                    expected[r] / total * 2.8
                }
                _ => cards * 0.4,
            };
            let mut bonus = 1.0 + hand_term + f64::from(public_points(s, v)) * 0.3;
            if Some(v) == endgame_leader {
                bonus += w.endgame.block_bias * super::leaf::opponent_points(s, v, w) * 0.3;
            }
            scored.push((
                hex_score + bonus,
                Action::MoveRobber {
                    hex,
                    victim: Some(v),
                },
            ));
        }
    }
    // Never steal from a seat this seat wants to keep trading with while any placement
    // robbing a threatening seat exists.
    if let Some(threats) = &threats {
        let has_safe_victim = scored.iter().any(|(_, a)| {
            matches!(
                a,
                Action::MoveRobber {
                    victim: Some(v),
                    ..
                } if threats[*v as usize].total >= 0.5
            )
        });
        if has_safe_victim {
            scored.retain(|(_, a)| {
                !matches!(
                    a,
                    Action::MoveRobber {
                        victim: Some(v),
                        ..
                    } if threats[*v as usize].total < 0.5
                )
            });
        }
    }
    scored
}
/// The resource this seat most needs: the largest missing-card gap toward the builds it
/// is closest to affording, tie-broken by the leaf's resource values. None when the hand
/// already affords every build.
pub fn needed_resource(s: &State, p: u8, w: &Weights) -> Option<usize> {
    let own = s.players[p as usize].resources;
    let mut need = [0.0f64; 5];
    for cost in [
        Resources::CITY,
        Resources::SETTLEMENT,
        Resources::DEVELOPMENT,
        Resources::ROAD,
    ] {
        let weight = 1.0 / (1.0 + crate::leaf::deficit(own, cost));
        for (r, &missing) in cost.0.iter().enumerate() {
            need[r] += f64::from(missing.saturating_sub(own.0[r])) * weight;
        }
    }
    (0..5)
        .max_by(|&a, &b| {
            need[a]
                .total_cmp(&need[b])
                .then(w.resource_value[a].total_cmp(&w.resource_value[b]))
        })
        .filter(|&r| need[r] > 0.0)
}
/// The 11 dice totals with their exact probabilities out of 36.
pub fn dice_totals() -> [(u8, f64); 11] {
    std::array::from_fn(|i| {
        let total = i as u8 + 2;
        (total, f64::from(6 - total.abs_diff(7)) / 36.0)
    })
}
fn dice_chance(total: u8) -> Chance {
    let first = total.saturating_sub(6).max(1);
    Chance::Dice {
        first,
        second: total - first,
    }
}
fn stream(seed: u32, salt: u32) -> Random {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&seed.to_le_bytes());
    bytes[4..8].copy_from_slice(&salt.to_le_bytes());
    bytes[8..12].copy_from_slice(b"exp2");
    Random::new(bytes)
}
fn pick_weighted(rng: &mut Random, weights: &[u32]) -> usize {
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
    rng: Random,
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
    fn roll(&mut self) -> u8 {
        let total = if self.cursor < self.rolls.len() {
            self.rolls[self.cursor]
        } else {
            (self.rng.below(6) + self.rng.below(6) + 2) as u8
        };
        self.cursor += 1;
        total
    }
    fn reset(&mut self) {
        self.cursor = 0;
        // Reuse the same stratified rolls for every world and afterstate.
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

    fn useful_roads(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        road_preference(s, p, &self.w, cap)
    }
    fn robber_candidates(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        robber_preference_with(
            s,
            p,
            &self.w,
            cap,
            self.limits.robber,
            self.limits.expected_hands,
            self.limits.predict_threat,
        )
    }
    /// Whether a road, settlement, or city is affordable now or within the next own turn at
    /// current production, using the leaf's acquisition-time estimate. The `buy_tempo` rule
    /// offers a development purchase only when this is false.
    fn build_within_turn(&self, s: &State, p: u8) -> bool {
        let own = s.players[p as usize];
        let rates = super::leaf::rates(s, p);
        let soon = |cost: Resources| super::leaf::eta_turns(s, own.resources, cost, &rates) <= 1.0;
        (own.cities < 4 && (0..54u8).any(|v| s.board.building(v) == p + 1) && soon(Resources::CITY))
            || (own.settlements < 5 && soon(Resources::SETTLEMENT))
            || (own.roads < 15 && soon(Resources::ROAD))
    }
    /// Whether the robber sits on this seat's best-producing hex: the one a held knight is
    /// saved to unblock before the roll under the `hold_knight` rule.
    fn robber_on_best(&self, s: &State, p: u8) -> bool {
        let mut best: Option<(f64, u8)> = None;
        for hex in 0..19u8 {
            let Some(resource) = s.layout.resource(hex) else {
                continue;
            };
            let pips = f64::from(6u8.saturating_sub(s.layout.number(hex).abs_diff(7)));
            let mut rate = 0.0;
            for &v in &topology().hex_vertices[hex as usize] {
                if s.board.owner(v) == Some(p) {
                    rate += pips / 36.0 * if s.board.building(v) > 4 { 2.0 } else { 1.0 };
                }
            }
            let value = rate * self.w.resource_value[resource as usize];
            if value > 0.0 && best.as_ref().is_none_or(|b| value > b.0) {
                best = Some((value, hex));
            }
        }
        best.is_some_and(|(_, hex)| hex == s.robber)
    }
    fn deficits(own: Resources) -> [(Resources, f64); 4] {
        [
            Resources::CITY,
            Resources::SETTLEMENT,
            Resources::DEVELOPMENT,
            Resources::ROAD,
        ]
        .map(|cost| (cost, crate::leaf::deficit(own, cost)))
    }
    /// Sum of the four build deficits with the kept hand credited by `rates * rolls` of
    /// expected production per resource.
    fn deficits_projected(kept: Resources, rates: &[f64; 5], rolls: f64) -> f64 {
        [
            Resources::CITY,
            Resources::SETTLEMENT,
            Resources::DEVELOPMENT,
            Resources::ROAD,
        ]
        .iter()
        .map(|cost| {
            (0..5)
                .map(|r| {
                    let have = f64::from(kept.0[r]) + rates[r] * rolls;
                    (f64::from(cost.0[r]) - have).max(0.0)
                })
                .sum::<f64>()
        })
        .sum()
    }
    fn bank_trades(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        let own = s.players[p as usize].resources;
        let mut scored: Vec<(f64, Action)> = Vec::new();
        for give in Resource::ALL {
            let ratio = trade_ratio(s, p, give);
            if own.0[give as usize] < ratio {
                continue;
            }
            for get in Resource::ALL {
                if get == give || s.bank.0[get as usize] == 0 {
                    continue;
                }
                let after = Resources(std::array::from_fn(|r| {
                    own.0[r] - if r == give as usize { ratio } else { 0 }
                        + u8::from(r == get as usize)
                }));
                let gain = Self::deficits(own)
                    .iter()
                    .zip(Self::deficits(after))
                    .map(|(before, later)| before.1 - later.1)
                    .fold(0.0, f64::max);
                if gain > 0.0 {
                    scored.push((
                        gain * 10.0 - f64::from(ratio),
                        Action::BankTrade {
                            give: Resources::one(give, ratio),
                            receive: Resources::one(get, 1),
                        },
                    ));
                }
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(cap).map(|(_, a)| a).collect()
    }
    /// Extra candidates for a hand over the discard limit: bank trades and roads chosen only
    /// to shed cards before another seat's roll. The leaf's `hand_risk` term prices shedding
    /// against holding; this switch only puts the choices on the list.
    fn spend_down(&self, s: &State, p: u8, out: &mut Vec<Action>) {
        let own = s.players[p as usize];
        let hand = own.resources;
        let mut trades: Vec<(f64, Action)> = Vec::new();
        for give in Resource::ALL {
            let ratio = trade_ratio(s, p, give);
            if hand.0[give as usize] < ratio {
                continue;
            }
            for get in Resource::ALL {
                if get == give || s.bank.0[get as usize] == 0 {
                    continue;
                }
                let after = Resources(std::array::from_fn(|r| {
                    hand.0[r] - if r == give as usize { ratio } else { 0 }
                        + u8::from(r == get as usize)
                }));
                let gain = Self::deficits(hand)
                    .iter()
                    .zip(Self::deficits(after))
                    .map(|(before, later)| before.1 - later.1)
                    .fold(0.0, f64::max);
                // Rank by cards shed, then by the deficit change.
                trades.push((
                    f64::from(ratio - 1) * 10.0 + gain,
                    Action::BankTrade {
                        give: Resources::one(give, ratio),
                        receive: Resources::one(get, 1),
                    },
                ));
            }
        }
        trades.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut added = 0;
        for (_, a) in trades {
            if added == 3 {
                break;
            }
            if !out.contains(&a) {
                out.push(a);
                added += 1;
            }
        }
        if own.roads < 15 && hand.contains(Resources::ROAD) {
            let (base_best, _) = super::leaf::reach(s, p);
            let mut roads: Vec<(f64, u8)> = Vec::new();
            for e in 0..72u8 {
                if !road_allowed(s, p, e) {
                    continue;
                }
                let Ok(next) = apply(s, p, Action::BuildRoad { edge: e }) else {
                    continue;
                };
                let (best, others) = super::leaf::reach(&next, p);
                roads.push((best - base_best + f64::from(others) * 0.02, e));
            }
            roads.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut added = 0;
            for (_, e) in roads {
                if added == 2 {
                    break;
                }
                let a = Action::BuildRoad { edge: e };
                if !out.contains(&a) {
                    out.push(a);
                    added += 1;
                }
            }
        }
    }
    fn plenty_candidates(s: &State, p: u8) -> Vec<Action> {
        let own = s.players[p as usize].resources;
        let count = s.bank.total().min(2);
        let mut out = Vec::new();
        for i in 0..5 {
            for j in i..5 {
                let mut r = Resources::default();
                r.0[i] += 1;
                if count == 2 {
                    r.0[j] += 1;
                } else if j != i {
                    continue;
                }
                if !s.bank.contains(r) {
                    continue;
                }
                let after = Resources(std::array::from_fn(|k| own.0[k] + r.0[k]));
                let gain = Self::deficits(own)
                    .iter()
                    .zip(Self::deficits(after))
                    .map(|(b, l)| b.1 - l.1)
                    .fold(0.0, f64::max);
                out.push((gain, Action::PlayYearOfPlenty { resources: r }));
                if count < 2 {
                    break;
                }
            }
        }
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter().take(2).map(|(_, a)| a).collect()
    }
    /// Discard choices for the root player: every multiset of the required size, ranked by
    /// how many target-build cards survive. Returns at most `cap`. With `project`, the kept
    /// hand is first credited with the seat's expected production over the rolls before its
    /// next turn, so a discard is valued by the target it preserves after that production
    /// rather than by the current hand alone.
    pub fn discard_candidates(s: &State, p: u8, cap: usize, project: bool) -> Vec<Action> {
        let hand = s.players[p as usize].resources;
        let k = hand.total() / 2;
        // Non-seven rolls before the seat's next turn, at most one per seat.
        let rolls = if project {
            f64::from(s.settings.players) * 5.0 / 6.0
        } else {
            0.0
        };
        let rates = if project {
            super::leaf::rates(s, p)
        } else {
            [0.0; 5]
        };
        let mut picks: Vec<Resources> = Vec::new();
        let mut pick = Resources::default();
        fn rec(
            i: usize,
            left: u16,
            hand: Resources,
            pick: &mut Resources,
            out: &mut Vec<Resources>,
        ) {
            if i == 5 {
                if left == 0 {
                    out.push(*pick);
                }
                return;
            }
            let max = u16::from(hand.0[i]).min(left);
            for n in 0..=max {
                pick.0[i] = n as u8;
                rec(i + 1, left - n, hand, pick, out);
            }
            pick.0[i] = 0;
        }
        rec(0, k, hand, &mut pick, &mut picks);
        let mut out: Vec<(f64, Resources)> = picks
            .into_iter()
            .map(|pick| {
                let kept = Resources(std::array::from_fn(|r| hand.0[r] - pick.0[r]));
                let missing = if project {
                    Search::deficits_projected(kept, &rates, rolls)
                } else {
                    Search::deficits(kept).iter().map(|d| d.1).sum::<f64>()
                };
                let diversity = f64::from(kept.0.iter().filter(|&&n| n > 0).count() as u8);
                (-missing + diversity * 0.1, pick)
            })
            .collect();
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter()
            .take(cap)
            .map(|(_, r)| Action::Discard { resources: r })
            .collect()
    }
    /// Actions worth considering for the root player in world `s`.
    pub fn candidates(&self, s: &State, p: u8, steps: u32) -> Vec<Action> {
        let own = s.players[p as usize];
        let mut out = Vec::new();
        match s.phase {
            Phase::SetupSettlement { .. } => {
                if self.limits.opening != super::OpeningMode::Search {
                    return super::opening::rank_settlements(
                        s,
                        p,
                        1,
                        &super::opening::Bias::default(),
                    )
                    .into_iter()
                    .take(6)
                    .map(|plan| Action::BuildSettlement {
                        vertex: plan.vertex,
                    })
                    .collect();
                }
                let mut scored: Vec<(f64, u8)> = (0..54u8)
                    .filter(|&v| settlement_allowed(s, p, v, true))
                    .map(|v| {
                        let next = apply(s, p, Action::BuildSettlement { vertex: v }).ok();
                        let value = next.map_or(0.0, |n| self.value(&n, p).total());
                        (value, v)
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.total_cmp(&a.0));
                return scored
                    .into_iter()
                    .take(6)
                    .map(|(_, v)| Action::BuildSettlement { vertex: v })
                    .collect();
            }
            Phase::SetupRoad { vertex, .. } => {
                if self.limits.opening != super::OpeningMode::Search {
                    return super::opening::rank_roads(s, p, vertex)
                        .into_iter()
                        .take(3)
                        .map(|(edge, _, _)| Action::BuildRoad { edge })
                        .collect();
                }
                return topology().incident_edges[vertex as usize]
                    .iter()
                    .filter(|&&e| s.board.road(e).is_none())
                    .map(|&e| Action::BuildRoad { edge: e })
                    .collect();
            }
            Phase::FreeRoads { remaining, .. } => {
                let mut roads = self.useful_roads(s, p, 3);
                if roads.is_empty() {
                    roads.extend(
                        (0..72u8)
                            .filter(|&e| road_allowed(s, p, e))
                            .take(2)
                            .map(|e| Action::BuildRoad { edge: e }),
                    );
                }
                if remaining == 1 {
                    roads.push(Action::FinishRoadBuilding);
                }
                return roads;
            }
            Phase::Discard { .. } => {
                return Self::discard_candidates(s, p, 1, self.limits.discard_projection);
            }
            Phase::Robber { .. } => return self.robber_candidates(s, p, 4),
            Phase::Finished { .. } => return vec![],
            Phase::Roll => {
                out.push(Action::Roll);
                if !s.played_development && own.development[0] > 0 {
                    out.push(Action::PlayKnight);
                }
                return out;
            }
            Phase::Main => {}
        }
        if steps >= 12 {
            return vec![Action::EndTurn];
        }
        out.push(Action::EndTurn);
        if own.cities < 4 && own.resources.contains(Resources::CITY) {
            for v in 0..54u8 {
                if s.board.building(v) == p + 1 {
                    out.push(Action::BuildCity { vertex: v });
                }
            }
        }
        if own.settlements < 5 && own.resources.contains(Resources::SETTLEMENT) {
            for v in 0..54u8 {
                if settlement_allowed(s, p, v, false) {
                    out.push(Action::BuildSettlement { vertex: v });
                }
            }
        }
        if !s.played_development {
            if own.development[Development::Knight as usize] > 0
                && (!self.limits.hold_knight || self.robber_on_best(s, p))
            {
                out.push(Action::PlayKnight);
            }
            if own.development[Development::RoadBuilding as usize] > 0
                && apply(s, p, Action::PlayRoadBuilding).is_ok()
            {
                out.push(Action::PlayRoadBuilding);
            }
            if own.development[Development::YearOfPlenty as usize] > 0 {
                out.extend(Self::plenty_candidates(s, p));
            }
            if own.development[Development::Monopoly as usize] > 0 {
                let mut best = (0u16, Resource::Brick);
                for r in Resource::ALL {
                    let total: u16 = (0..s.settings.players)
                        .filter(|&o| o != p)
                        .map(|o| u16::from(s.players[o as usize].resources.0[r as usize]))
                        .sum();
                    if total > best.0 {
                        best = (total, r);
                    }
                }
                if best.0 > 0 {
                    out.push(Action::PlayMonopoly { resource: best.1 });
                }
            }
        }
        if s.deck_count() > 0
            && own.resources.contains(Resources::DEVELOPMENT)
            && (!self.limits.buy_tempo || !self.build_within_turn(s, p))
        {
            out.push(Action::BuyDevelopment);
        }
        out.extend(self.useful_roads(s, p, 4));
        out.extend(self.bank_trades(s, p, 4));
        if self.limits.spend_down && own.resources.total() > s.settings.discard_limit as u16 {
            self.spend_down(s, p, &mut out);
        }
        out
    }

    // ----- one planned turn ---------------------------------------------------------------

    /// Actions to keep at a given step of a turn. The current turn keeps every first action,
    /// then `EndTurn` plus the best few by static value; later turns follow one greedy line.
    /// `None` means keep everything; `Some(0)` means the single statically best action.
    fn step_cap(&self, level: u8, steps: u32) -> Option<usize> {
        if level > 1 {
            return Some(0);
        }
        match steps {
            0 => None,
            1 => Some(3),
            2 => Some(2),
            3 => Some(1),
            _ => Some(usize::MAX),
        }
    }
    /// The statically best continuation of the root player's turn from a group: the maximum
    /// over own actions of the expectation over chance outcomes, valued by the leaf at every
    /// end-of-turn position. Returns the value and the afterstate groups of the best line,
    /// each with its probability mass (chance inside the turn splits the line).
    pub fn best_line(
        &mut self,
        worlds: Vec<World>,
        level: u8,
        steps: u32,
    ) -> Result<(f64, Line), RuleError> {
        let s = worlds[0].state;
        let key = information_set(&s, self.seat);
        if let Some((value, line, generation, ticks)) = self.cache.lines.get(&key) {
            let (hit, generation, ticks) = ((*value, line.clone()), *generation, *ticks);
            self.stats.transpositions += 1;
            self.reuse(generation, ticks);
            return Ok(hit);
        }
        let ticks_before = self.stats.transitions;
        let worlds = Self::normalized(worlds);
        let p = actor(&s);
        let ended = matches!(s.phase, Phase::Finished { .. })
            || (p != self.seat && !matches!(s.phase, Phase::Discard { .. }));
        if ended || self.aborted {
            let value = self.leaf(&worlds).total();
            return Ok((value, vec![(1.0, worlds)]));
        }
        if p != self.seat {
            // Another seat must discard before our turn continues.
            let mut children = Vec::with_capacity(worlds.len());
            for w in &worlds {
                let a = opponent::act(&w.state, p, 0);
                children.push(World {
                    state: apply(&w.state, p, a)?,
                    weight: w.weight,
                });
                self.tick(1);
            }
            let mut value = 0.0;
            let mut lines = Vec::new();
            for (_, group) in self.regroup(children) {
                let mass = Self::mass(&group);
                let (v, sub) = self.best_line(group, level, steps)?;
                value += mass * v;
                lines.extend(sub.into_iter().map(|(m, g)| (m * mass, g)));
            }
            return Ok((value, lines));
        }
        self.note_node(level);
        let actions = match self.step_cap(level, steps) {
            None => self.candidates(&s, p, steps),
            Some(usize::MAX) => vec![Action::EndTurn],
            Some(cap) => {
                let all = self.candidates(&s, p, steps);
                self.narrow(&s, p, all, cap)
            }
        };
        let mut best: Option<(f64, Line)> = None;
        for a in actions {
            let children = match self.step(&worlds, p, a) {
                Ok(children) => children,
                Err(_) => continue,
            };
            let mut value = 0.0;
            let mut lines = Vec::new();
            for (_, group) in self.regroup(children) {
                let mass = Self::mass(&group);
                let (v, sub) = self.best_line(group, level, steps + 1)?;
                value += mass * v;
                lines.extend(sub.into_iter().map(|(m, g)| (m * mass, g)));
            }
            if best.as_ref().is_none_or(|b| value > b.0) {
                best = Some((value, lines));
            }
        }
        let result = best.unwrap_or_else(|| (self.leaf(&worlds).total(), vec![(1.0, worlds)]));
        if !self.aborted {
            let ticks = self.stats.transitions - ticks_before;
            self.cache
                .lines
                .insert(key, (result.0, result.1.clone(), self.generation, ticks));
        }
        Ok(result)
    }
    /// Keep the `cap` actions whose immediate static value is highest, always keeping
    /// `EndTurn` when `cap` is positive; `cap == 0` keeps only the single best action.
    fn narrow(&mut self, s: &State, p: u8, actions: Vec<Action>, cap: usize) -> Vec<Action> {
        if actions.len() <= cap + 1 {
            return actions;
        }
        let here = self.value(s, self.seat).total();
        self.stats.leaves += 1;
        let mut scored: Vec<(f64, Action)> = actions
            .into_iter()
            .map(|a| {
                let value = if a == Action::EndTurn {
                    if cap > 0 { f64::INFINITY } else { here }
                } else {
                    match apply(s, p, a) {
                        Ok(next) => {
                            self.stats.leaves += 1;
                            self.value(&next, self.seat).total()
                        }
                        Err(_) => f64::NEG_INFINITY,
                    }
                };
                (value, a)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(cap.max(1) + usize::from(cap > 0))
            .map(|(_, a)| a)
            .collect()
    }
    /// Value of the root player's turn with lookahead: the statically best line's afterstates
    /// are followed through the opponents' round and the next planned level. At the deepest
    /// level the static value is the answer.
    pub fn turn_value(
        &mut self,
        worlds: Vec<World>,
        level: u8,
        steps: u32,
    ) -> Result<(Terms, Option<Action>), RuleError> {
        let key = (
            information_set(&worlds[0].state, self.seat),
            level,
            self.limits.depth,
        );
        if let Some((v, recorded, generation, ticks)) = self.cache.memo.get(&key) {
            let (v, recorded, generation, ticks) =
                (v.clone(), recorded.clone(), *generation, *ticks);
            self.stats.transpositions += 1;
            self.reuse(generation, ticks);
            if self.record {
                for (share, value, state) in recorded {
                    self.note_outcome(level, share, value, state);
                }
            }
            return Ok((v, None));
        }
        let ticks_before = self.stats.transitions;
        let worlds = Self::normalized(worlds);
        let (_, lines) = self.best_line(worlds.clone(), level, steps)?;
        let mass_total: f64 = lines
            .iter()
            .filter(|(m, _)| *m > 1e-12)
            .map(|(m, _)| m)
            .sum();
        let mut total = Terms::default();
        let mut recorded: Recorded = Vec::new();
        for (mass, group) in lines {
            if mass <= 1e-12 {
                continue;
            }
            let group = Self::normalized(group);
            let share = mass / mass_total;
            let saved = self.path_mass;
            self.path_mass *= share;
            let value = self.afterstate_value(&group, level);
            self.path_mass = saved;
            let value = value?;
            if self.record {
                recorded.push((share, value.total(), group[0].state));
                self.note_outcome(level, share, value.total(), group[0].state);
            }
            total.add_scaled(&value, mass);
        }
        if mass_total > 0.0 {
            total.scale(1.0 / mass_total);
        } else {
            total = self.leaf(&worlds);
        }
        if !self.aborted {
            let ticks = self.stats.transitions - ticks_before;
            self.cache
                .memo
                .insert(key, (total.clone(), recorded, self.generation, ticks));
        }
        Ok((total, None))
    }
    fn afterstate_value(&mut self, worlds: &[World], level: u8) -> Result<Terms, RuleError> {
        self.note_afterstate(level);
        if level >= self.limits.depth
            || matches!(worlds[0].state.phase, Phase::Finished { .. })
            || self.aborted
        {
            return Ok(self.leaf(worlds));
        }
        self.stats.expanded += 1;
        self.round(worlds, level)
    }
    /// Plan a whole turn from its first decision.
    pub fn plan(
        &mut self,
        worlds: Vec<World>,
        level: u8,
    ) -> Result<(Terms, Option<Action>), RuleError> {
        self.turn_value(worlds, level, 0)
    }
    fn note_node(&mut self, level: u8) {
        let i = level as usize;
        if self.stats.per_level_nodes.len() <= i {
            self.stats.per_level_nodes.resize(i + 1, 0);
            self.stats.per_level_afterstates.resize(i + 1, 0);
        }
        self.stats.per_level_nodes[i] += 1;
        self.stats.own_nodes += 1;
    }
    fn note_afterstate(&mut self, level: u8) {
        let i = level as usize;
        if self.stats.per_level_nodes.len() <= i {
            self.stats.per_level_nodes.resize(i + 1, 0);
            self.stats.per_level_afterstates.resize(i + 1, 0);
        }
        self.stats.per_level_afterstates[i] += 1;
        self.stats.afterstates += 1;
    }

    /// An opponent's move under the leaf-guided model: the candidate whose immediate
    /// static value from that seat's perspective is highest, with `EndTurn` after long turns.
    fn leaf_action(&mut self, s: &State, p: u8, steps: u32) -> Action {
        if steps >= 12 && s.phase == Phase::Main {
            return Action::EndTurn;
        }
        let candidates = match s.phase {
            Phase::Main | Phase::Roll | Phase::Robber { .. } | Phase::FreeRoads { .. } => {
                self.candidates(s, p, steps)
            }
            _ => vec![opponent::act(s, p, steps)],
        };
        let mut best: Option<(f64, Action)> = None;
        for a in candidates {
            let value = match a {
                Action::Roll => f64::INFINITY,
                Action::EndTurn => self.value(s, p).total(),
                _ => match apply(s, p, a) {
                    Ok(next) => self.value(&next, p).total(),
                    Err(_) => continue,
                },
            };
            self.stats.leaves += 1;
            if best.as_ref().is_none_or(|b| value > b.0) {
                best = Some((value, a));
            }
        }
        best.map_or_else(|| opponent::act(s, p, steps), |b| b.1)
    }

    // ----- opponents' round under common random scenarios ------------------------------------

    /// Arrival positions after every other seat's turn, for every scenario of the level, as
    /// (mass share, group) in scenario then group order. Cached per afterstate group and level
    /// across the depth passes of a decision; `None` when the search aborted mid-round.
    fn arrivals(&mut self, worlds: &[World], level: u8) -> Result<Option<Arrivals>, RuleError> {
        let count = if level <= 1 {
            self.limits.scenarios
        } else {
            self.limits.inner_scenarios
        }
        .max(1);
        let key = (information_set(&worlds[0].state, self.seat), level);
        if let Some((arrivals, generation, ticks)) = self.cache.rounds.get(&key) {
            let (arrivals, generation, ticks) = (arrivals.clone(), *generation, *ticks);
            self.stats.scenarios += count as u64;
            self.reuse(generation, ticks);
            return Ok(Some(arrivals));
        }
        let ticks_before = self.stats.transitions;
        let players = worlds[0].state.settings.players as usize;
        let mut scenarios = Scenario::set(count, u32::from(level), players);
        let mut arrivals: Arrivals = Vec::new();
        for scenario in &mut scenarios {
            self.stats.scenarios += 1;
            let mut arrived = Vec::with_capacity(worlds.len());
            for w in worlds {
                scenario.reset();
                let mut s = w.state;
                let mut steps = 0u32;
                let mut turn_steps = 0u32;
                let mut turn_key = (s.turn, s.current);
                while !matches!(s.phase, Phase::Finished { .. })
                    && !(s.current == self.seat && s.phase == Phase::Main && actor(&s) == self.seat)
                    && steps < MAX_STEPS
                {
                    if (s.turn, s.current) != turn_key {
                        turn_key = (s.turn, s.current);
                        turn_steps = 0;
                    }
                    let p = actor(&s);
                    let a = if p != self.seat && self.limits.predict_opponents {
                        super::predict::act(&s, p, turn_steps)
                    } else if self.limits.leaf_opponents && p != self.seat {
                        self.leaf_action(&s, p, turn_steps)
                    } else {
                        opponent::act(&s, p, turn_steps)
                    };
                    let next = match a {
                        Action::Roll => {
                            let total = scenario.roll();
                            apply_with_chance(&s, p, a, dice_chance(total))
                        }
                        Action::BuyDevelopment
                        | Action::MoveRobber {
                            victim: Some(_), ..
                        } => {
                            let branches = chance_outcomes(&s, p, a)?;
                            let weights: Vec<u32> = branches.iter().map(|b| b.numerator).collect();
                            let i = pick_weighted(&mut scenario.rng, &weights);
                            apply_with_chance(&s, p, a, branches[i].outcome)
                        }
                        _ => apply(&s, p, a),
                    };
                    match next {
                        Ok(n) => s = n,
                        Err(_) => {
                            // The builder proposed an illegal move in this world; end the turn.
                            let fallback = match s.phase {
                                Phase::Main => Action::EndTurn,
                                Phase::Roll => Action::Roll,
                                _ => *legal_actions(&s, p).first().ok_or(RuleError::WrongPhase)?,
                            };
                            s = match fallback {
                                Action::Roll => apply_with_chance(
                                    &s,
                                    p,
                                    fallback,
                                    dice_chance(scenario.roll()),
                                )?,
                                _ => apply(&s, p, fallback)?,
                            };
                        }
                    }
                    steps += 1;
                    turn_steps += 1;
                }
                self.tick(u64::from(steps));
                arrived.push(World {
                    state: s,
                    weight: w.weight,
                });
                if self.aborted {
                    return Ok(None);
                }
            }
            for (_, group) in self.regroup(arrived) {
                let share = Self::mass(&group) / count as f64;
                arrivals.push((share, group));
            }
        }
        let ticks = self.stats.transitions - ticks_before;
        self.cache
            .rounds
            .insert(key, (arrivals.clone(), self.generation, ticks));
        Ok(Some(arrivals))
    }
    /// Simulate from an afterstate through every other seat's turn to the root player's next
    /// main phase, then plan the next level. Averages over the level's scenarios.
    pub fn round(&mut self, worlds: &[World], level: u8) -> Result<Terms, RuleError> {
        let Some(arrivals) = self.arrivals(worlds, level)? else {
            return Ok(Terms::default());
        };
        let mut total = Terms::default();
        for (share, group) in arrivals {
            let finished = matches!(group[0].state.phase, Phase::Finished { .. });
            let value = if finished || level + 1 > self.limits.depth {
                let state = group[0].state;
                let value = self.leaf(&Self::normalized(group));
                if finished && level < self.limits.depth {
                    self.note_outcome(level + 1, share, value.total(), state);
                }
                value
            } else {
                let saved = self.path_mass;
                self.path_mass *= share;
                let planned = self.plan(group, level + 1);
                self.path_mass = saved;
                planned?.0
            };
            total.add_scaled(&value, share);
        }
        Ok(total)
    }
}

#[cfg(test)]
#[path = "../../tests/internal/v2_robber.rs"]
mod robber_tests;

#[cfg(test)]
#[path = "../../tests/internal/road_block.rs"]
mod road_block_tests;

#[cfg(test)]
#[path = "../../tests/internal/road_contention.rs"]
mod road_contention_tests;
