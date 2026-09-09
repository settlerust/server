//! Turn-level expectimax over sampled worlds.
//!
//! A level is one of the root player's turns. Within a level the search enumerates distinct
//! end-of-turn positions (afterstates) reachable by action sequences, with exact chance branches
//! for purchases, thefts, and the player's own roll. Between levels, opponents play a fixed
//! builder policy under common random scenarios: every afterstate and every root candidate sees
//! the same stratified dice, so comparisons are paired. Leaves are evaluated at the end of the
//! deepest planned turn. Iterative deepening keeps the deepest fully completed level count.
use super::key::information_set;
use super::leaf::{Terms, evaluate};
use super::opponent;
use super::weights::Weights;
use serde::{Deserialize, Serialize};
use settlers_engine::*;
use std::collections::{BTreeMap, HashMap, HashSet};

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
    pub scenarios: u64,
    pub own_nodes: u64,
    pub per_level_nodes: Vec<u64>,
    pub per_level_afterstates: Vec<u64>,
    pub aborted: bool,
}
pub struct Limits {
    pub depth: u8,
    pub beam: usize,
    pub root_beam: usize,
    pub per_candidate: usize,
    pub afterstates: usize,
    pub scenarios: usize,
    pub inner_scenarios: usize,
    pub node_budget: u64,
}
pub struct Search<'a> {
    pub seat: u8,
    pub w: Weights,
    pub limits: Limits,
    pub stats: Stats,
    /// Returns true once the time budget is exhausted.
    pub over_time: &'a dyn Fn() -> bool,
    pub aborted: bool,
    memo: HashMap<(u64, u8), Terms>,
}
pub struct Afterstate {
    pub worlds: Vec<World>,
    pub mass: f64,
    pub key: u64,
    pub static_value: f64,
    pub first: Option<Action>,
}
const MAX_STEPS: u32 = 400;

fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
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
    pub fn new(seat: u8, w: Weights, limits: Limits, over_time: &'a dyn Fn() -> bool) -> Self {
        Self {
            seat,
            w,
            limits,
            stats: Stats::default(),
            over_time,
            aborted: false,
            memo: HashMap::new(),
        }
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
            let terms = evaluate(&w.state, self.seat, &self.w);
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
    fn regroup(&self, worlds: Vec<World>) -> Vec<(u64, Vec<World>)> {
        let mut groups: BTreeMap<u64, Vec<World>> = BTreeMap::new();
        for w in worlds {
            if w.weight > 1e-12 {
                groups
                    .entry(information_set(&w.state, self.seat))
                    .or_default()
                    .push(w);
            }
        }
        groups.into_iter().collect()
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
        let own = s.players[p as usize];
        if own.roads >= 15
            || !s.players[p as usize].resources.contains(Resources::ROAD) && s.phase == Phase::Main
        {
            return vec![];
        }
        let mut scored: Vec<(f64, u8)> = Vec::new();
        let (base_best, _) = super::leaf::reach(s, p);
        let leader = (0..s.settings.players)
            .filter(|&o| o != p)
            .map(|o| road_length(s, o))
            .max()
            .unwrap_or(0)
            .max(4);
        let own_length = road_length(s, p);
        let contend = own.roads >= 4 && own_length + 2 >= leader && s.longest_road != Some(p);
        for e in 0..72u8 {
            if !road_allowed(s, p, e) {
                continue;
            }
            let Ok(next) = apply(s, p, Action::BuildRoad { edge: e }) else {
                continue;
            };
            let (best, others) = super::leaf::reach(&next, p);
            let mut score = best - base_best + f64::from(others) * 0.02;
            if contend && road_length(&next, p) > own_length {
                score += 0.5;
            }
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
    fn robber_candidates(&self, s: &State, p: u8, cap: usize) -> Vec<Action> {
        let mut scored: Vec<(f64, Action)> = Vec::new();
        for hex in 0..19u8 {
            if hex == s.robber {
                continue;
            }
            let pips = f64::from(6 - s.layout.number(hex).abs_diff(7)).max(0.0);
            let mut blocked = 0.0;
            let mut own = false;
            for &v in &topology().hex_vertices[hex as usize] {
                if let Some(o) = s.board.owner(v) {
                    let scale = if s.board.building(v) > 4 { 2.0 } else { 1.0 };
                    if o == p {
                        own = true;
                    } else {
                        blocked += pips * scale * (1.0 + f64::from(public_points(s, o)) * 0.15);
                    }
                }
            }
            if own || s.layout.resource(hex).is_none() {
                continue;
            }
            let victims = robber_victims(s, p, hex);
            if victims.is_empty() {
                scored.push((blocked, Action::MoveRobber { hex, victim: None }));
            }
            for v in victims {
                let cards = f64::from(s.players[v as usize].resources.total()).min(8.0);
                let bonus = 1.0 + cards * 0.4 + f64::from(public_points(s, v)) * 0.3;
                scored.push((
                    blocked + bonus,
                    Action::MoveRobber {
                        hex,
                        victim: Some(v),
                    },
                ));
            }
        }
        if scored.is_empty() {
            return vec![opponent::robber(s, p)];
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(cap).map(|(_, a)| a).collect()
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
    /// how many target-build cards survive. Returns at most `cap`.
    pub fn discard_candidates(s: &State, p: u8, cap: usize) -> Vec<Action> {
        let hand = s.players[p as usize].resources;
        let k = hand.total() / 2;
        let mut out: Vec<(f64, Resources)> = Vec::new();
        let mut pick = Resources::default();
        fn rec(
            i: usize,
            left: u16,
            hand: Resources,
            pick: &mut Resources,
            out: &mut Vec<(f64, Resources)>,
        ) {
            if i == 5 {
                if left == 0 {
                    let kept = Resources(std::array::from_fn(|r| hand.0[r] - pick.0[r]));
                    let score = -Search::deficits(kept).iter().map(|d| d.1).sum::<f64>()
                        + f64::from(kept.0.iter().filter(|&&n| n > 0).count() as u8) * 0.1;
                    out.push((score, *pick));
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
        rec(0, k, hand, &mut pick, &mut out);
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
                let mut scored: Vec<(f64, u8)> = (0..54u8)
                    .filter(|&v| settlement_allowed(s, p, v, true))
                    .map(|v| {
                        let next = apply(s, p, Action::BuildSettlement { vertex: v }).ok();
                        let value = next.map_or(0.0, |n| evaluate(&n, p, &self.w).total());
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
            Phase::Discard { .. } => return Self::discard_candidates(s, p, 1),
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
        if steps >= 24 {
            return vec![Action::EndTurn];
        }
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
            if own.development[Development::Knight as usize] > 0 {
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
        if s.deck_count() > 0 && own.resources.contains(Resources::DEVELOPMENT) {
            out.push(Action::BuyDevelopment);
        }
        out.extend(self.useful_roads(s, p, 4));
        out.extend(self.bank_trades(s, p, 4));
        out.push(Action::EndTurn);
        out
    }

    // ----- one planned turn ---------------------------------------------------------------

    /// Enumerate afterstates of the root player's turn from a group sharing one information
    /// set. `first` restricts the first action. Chance splits groups; opponents' pending
    /// discards resolve with the builder rule.
    pub fn enumerate(
        &mut self,
        worlds: Vec<World>,
        first: Option<Action>,
        out: &mut Vec<Afterstate>,
        visited: &mut HashSet<u64>,
        steps: u32,
    ) -> Result<(), RuleError> {
        if self.aborted || out.len() >= self.limits.afterstates {
            return Ok(());
        }
        let s = worlds[0].state;
        let key = information_set(&s, self.seat);
        if !visited.insert(key ^ u64::from(steps == 0)) {
            self.stats.transpositions += 1;
            return Ok(());
        }
        let mass = Self::mass(&worlds);
        if matches!(s.phase, Phase::Finished { .. }) {
            let value = self.leaf(&Self::normalized(worlds.clone())).total();
            out.push(Afterstate {
                worlds,
                mass,
                key,
                static_value: value,
                first,
            });
            return Ok(());
        }
        let p = actor(&s);
        if p != self.seat && !matches!(s.phase, Phase::Discard { .. }) {
            // Our turn has ended; this position is an afterstate.
            let value = self.leaf(&Self::normalized(worlds.clone())).total();
            out.push(Afterstate {
                worlds,
                mass,
                key,
                static_value: value,
                first,
            });
            return Ok(());
        }
        if p != self.seat {
            // Another seat must discard before our turn continues.
            let mut children = Vec::new();
            for w in &worlds {
                let a = opponent::act(&w.state, p, 0);
                children.push(World {
                    state: apply(&w.state, p, a)?,
                    weight: w.weight,
                });
                self.tick(1);
            }
            for (_, group) in self.regroup(children) {
                self.enumerate(group, first, out, visited, steps)?;
            }
            return Ok(());
        }
        self.stats.own_nodes += 1;
        let actions = match first {
            Some(a) => vec![a],
            None => self.candidates(&s, p, steps),
        };
        for a in actions {
            if self.aborted {
                return Ok(());
            }
            let children = match self.step(&worlds, p, a) {
                Ok(children) => children,
                Err(_) => continue,
            };
            let ends_turn = children.first().is_some_and(|w| {
                w.state.current != self.seat || matches!(w.state.phase, Phase::Finished { .. })
            });
            if ends_turn {
                for (k, group) in self.regroup(children) {
                    if !visited.insert(k) {
                        self.stats.transpositions += 1;
                        continue;
                    }
                    let mass = Self::mass(&group);
                    let value = self.leaf(&Self::normalized(group.clone())).total();
                    out.push(Afterstate {
                        worlds: group,
                        mass,
                        key: k,
                        static_value: value,
                        first: first.or(Some(a)),
                    });
                    if out.len() >= self.limits.afterstates {
                        return Ok(());
                    }
                }
            } else {
                for (_, group) in self.regroup(children) {
                    self.enumerate(group, first.or(Some(a)), out, visited, steps + 1)?;
                }
            }
        }
        Ok(())
    }

    /// Expected value from a group at the start of the root player's decision, planning
    /// `level` as the current level. Returns the value and the chosen first action.
    pub fn plan(
        &mut self,
        worlds: Vec<World>,
        level: u8,
    ) -> Result<(Terms, Option<Action>), RuleError> {
        let key = (information_set(&worlds[0].state, self.seat), level);
        if let Some(v) = self.memo.get(&key) {
            self.stats.transpositions += 1;
            return Ok((v.clone(), None));
        }
        let worlds = Self::normalized(worlds);
        let mut afterstates = Vec::new();
        let mut visited = HashSet::new();
        self.enumerate(worlds.clone(), None, &mut afterstates, &mut visited, 0)?;
        self.note_level(level, afterstates.len());
        if afterstates.is_empty() {
            let v = self.leaf(&worlds);
            return Ok((v, None));
        }
        let value = self.resolve(afterstates, level, self.limits.beam, 1)?;
        self.memo.insert(key, value.0.clone());
        Ok(value)
    }
    fn note_level(&mut self, level: u8, count: usize) {
        let i = level as usize;
        if self.stats.per_level_nodes.len() <= i {
            self.stats.per_level_nodes.resize(i + 1, 0);
            self.stats.per_level_afterstates.resize(i + 1, 0);
        }
        self.stats.per_level_nodes[i] += 1;
        self.stats.per_level_afterstates[i] += count as u64;
        self.stats.afterstates += count as u64;
    }
    /// Expand the best afterstates and return the value of the best first action.
    /// Afterstates sharing a first action combine by probability mass (chance within the turn).
    pub fn resolve(
        &mut self,
        afterstates: Vec<Afterstate>,
        level: u8,
        beam: usize,
        per_first: usize,
    ) -> Result<(Terms, Option<Action>), RuleError> {
        // Group afterstates by first action; each group is a distribution over afterstates.
        let mut by_first: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, a) in afterstates.iter().enumerate() {
            let label = serde_json::to_string(&a.first).unwrap_or_default();
            by_first.entry(label).or_default().push(i);
        }
        // Static value of each first action = mass-weighted static afterstate values.
        let mut static_first: Vec<(f64, String)> = by_first
            .iter()
            .map(|(label, ids)| {
                let mass: f64 = ids.iter().map(|&i| afterstates[i].mass).sum();
                let v: f64 = ids
                    .iter()
                    .map(|&i| afterstates[i].static_value * afterstates[i].mass)
                    .sum::<f64>()
                    / mass.max(1e-12);
                (v, label.clone())
            })
            .collect();
        static_first.sort_by(|a, b| b.0.total_cmp(&a.0));
        let last = level >= self.limits.depth;
        let mut chosen: HashSet<usize> = HashSet::new();
        if !last {
            for (_, label) in static_first.iter().take(beam) {
                for &i in &by_first[label] {
                    chosen.insert(i);
                }
            }
            // Every first action keeps at least its best afterstates so root candidates are
            // all searched, not only the statically best ones.
            for ids in by_first.values() {
                let mut ranked: Vec<usize> = ids.clone();
                ranked.sort_by(|&a, &b| {
                    afterstates[b]
                        .static_value
                        .total_cmp(&afterstates[a].static_value)
                });
                for &i in ranked.iter().take(per_first) {
                    chosen.insert(i);
                }
            }
        }
        let mut values: Vec<Option<Terms>> = vec![None; afterstates.len()];
        for (i, a) in afterstates.iter().enumerate() {
            if last {
                values[i] = Some(self.leaf(&Self::normalized(a.worlds.clone())));
            } else if chosen.contains(&i) {
                if self.aborted {
                    return Ok((Terms::default(), None));
                }
                let v = self.round(&Self::normalized(a.worlds.clone()), level)?;
                self.stats.expanded += 1;
                values[i] = Some(v);
            }
        }
        let mut best: Option<(f64, Terms, Option<Action>)> = None;
        for (label, ids) in &by_first {
            let mut total = Terms::default();
            let mut mass = 0.0;
            let mut complete = true;
            for &i in ids {
                match &values[i] {
                    Some(v) => {
                        total.add_scaled(v, afterstates[i].mass);
                        mass += afterstates[i].mass;
                    }
                    None => complete = false,
                }
            }
            if mass <= 0.0 {
                continue;
            }
            // A first action whose worse afterstates were pruned is valued by its expanded
            // part only; pruning removes low static branches, so this is optimistic and rare.
            let _ = complete;
            total.scale(1.0 / mass);
            let score = total.total();
            if best.as_ref().is_none_or(|b| score > b.0) {
                let first = afterstates[ids[0]].first;
                best = Some((score, total, first));
                let _ = label;
            }
        }
        Ok(best.map_or((Terms::default(), None), |b| (b.1, b.2)))
    }

    // ----- opponents' round under common random scenarios ------------------------------------

    /// Simulate from an afterstate through every other seat's turn to the root player's next
    /// main phase, then plan the next level. Averages over the level's scenarios.
    pub fn round(&mut self, worlds: &[World], level: u8) -> Result<Terms, RuleError> {
        let count = if level <= 1 {
            self.limits.scenarios
        } else {
            self.limits.inner_scenarios
        }
        .max(1);
        let players = worlds[0].state.settings.players as usize;
        let mut scenarios = Scenario::set(count, u32::from(level), players);
        let mut total = Terms::default();
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
                    let a = opponent::act(&s, p, turn_steps);
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
                    return Ok(Terms::default());
                }
            }
            for (_, group) in self.regroup(arrived) {
                let mass = Self::mass(&group);
                let finished = matches!(group[0].state.phase, Phase::Finished { .. });
                let value = if finished || level + 1 > self.limits.depth {
                    self.leaf(&Self::normalized(group))
                } else {
                    self.plan(group, level + 1)?.0
                };
                total.add_scaled(&value, mass / count as f64);
            }
        }
        Ok(total)
    }
}
