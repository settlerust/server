//! Opening placement planner. The two setup settlements are scored as a portfolio under
//! several discrete strategies (dice coverage, cities, roads, ports, cards); the picks that
//! the other seats make before this seat's next placement are predicted by the same scoring,
//! so a first placement is valued by the best pair it still leaves once the sites likely to
//! disappear have gone. Everything here reads only the public board and the seat's own seat
//! order; no hidden information exists during setup.
//!
//! Units are pips: one pip is one thirty-sixth of a card per roll, the value of a 2 or 12.
use settlers_engine::*;
use std::collections::BTreeMap;

/// A discrete opening strategy: how much each resource is worth, how badly a missing one
/// hurts, and how the port, expansion, coverage, and build-time terms are weighted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strategy {
    pub name: &'static str,
    /// Per-resource multipliers in brick, lumber, wool, grain, ore order; they sum to five.
    pub weights: [f64; 5],
    /// How much a resource that is missing or thin in the pair costs.
    pub need: [f64; 5],
    pub port: f64,
    pub expansion: f64,
    pub coverage: f64,
    /// Weight on the time to afford a settlement plus a road, and a city.
    pub settlement_tempo: f64,
    pub city_tempo: f64,
}
pub const STRATEGIES: [Strategy; 5] = [
    Strategy {
        name: "coverage",
        weights: [0.96, 0.96, 0.82, 1.11, 1.15],
        need: [0.8, 0.8, 0.6, 1.0, 1.0],
        port: 1.0,
        expansion: 1.0,
        coverage: 1.0,
        settlement_tempo: 1.0,
        city_tempo: 1.0,
    },
    Strategy {
        name: "cities",
        weights: [0.65, 0.65, 0.75, 1.4, 1.55],
        need: [0.2, 0.2, 0.4, 1.2, 1.3],
        port: 1.0,
        expansion: 0.6,
        coverage: 0.8,
        settlement_tempo: 0.6,
        city_tempo: 1.6,
    },
    Strategy {
        name: "roads",
        weights: [1.3, 1.3, 0.95, 0.9, 0.55],
        need: [1.2, 1.2, 0.8, 0.8, 0.2],
        port: 1.0,
        expansion: 1.6,
        coverage: 0.9,
        settlement_tempo: 1.6,
        city_tempo: 0.4,
    },
    Strategy {
        name: "ports",
        weights: [0.96, 0.96, 0.82, 1.11, 1.15],
        need: [0.5, 0.5, 0.4, 0.6, 0.6],
        port: 2.4,
        expansion: 0.8,
        coverage: 0.9,
        settlement_tempo: 0.9,
        city_tempo: 0.9,
    },
    Strategy {
        name: "cards",
        weights: [0.6, 0.6, 1.2, 1.25, 1.35],
        need: [0.2, 0.2, 1.1, 1.1, 1.1],
        port: 1.0,
        expansion: 0.5,
        coverage: 0.9,
        settlement_tempo: 0.5,
        city_tempo: 1.0,
    },
];

/// Named additive terms of a placement value, in pips.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Terms {
    pub production: f64,
    pub coverage: f64,
    pub balance: f64,
    pub tempo: f64,
    pub ports: f64,
    pub expansion: f64,
    /// The seat's inclination lean toward the strategy that values this pair.
    pub style: f64,
    /// Value lost to the picks predicted before this seat's next placement (never positive).
    pub draft: f64,
}
impl Terms {
    pub fn total(&self) -> f64 {
        self.production
            + self.coverage
            + self.balance
            + self.tempo
            + self.ports
            + self.expansion
            + self.style
            + self.draft
    }
    pub fn named(&self) -> BTreeMap<&'static str, f64> {
        BTreeMap::from([
            ("production", self.production),
            ("coverage", self.coverage),
            ("balance", self.balance),
            ("tempo", self.tempo),
            ("ports", self.ports),
            ("expansion", self.expansion),
            ("style", self.style),
            ("draft", self.draft),
        ])
    }
}
/// One ranked placement: the site, its value, the strategy that values it best, the second
/// site the plan expects to pair it with, and how many of the predicted drafts kept that site.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub vertex: u8,
    pub value: f64,
    pub terms: Terms,
    pub strategy: &'static str,
    pub partner: Option<u8>,
    pub survival: f64,
}
/// Multipliers on the planner's terms, so the balance between production and the
/// structural terms can be tuned and logged like the leaf weights.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OpeningWeights {
    pub production: f64,
    pub coverage: f64,
    pub balance: f64,
    pub tempo: f64,
    pub ports: f64,
    pub expansion: f64,
    /// How much of a specialized strategy's advantage over the coverage valuation counts:
    /// zero values every pair by coverage alone, one takes the best strategy in full.
    pub specialize: f64,
    /// How strongly resources that are scarce on the island are marked up (zero: not at all).
    pub scarcity: f64,
    /// Simulated drafts averaged for a first placement (the first is greedy).
    pub samples: u32,
}
impl Default for OpeningWeights {
    fn default() -> Self {
        Self {
            production: 1.0,
            coverage: 0.15,
            balance: 0.2,
            tempo: 1.0,
            ports: 1.0,
            expansion: 0.15,
            specialize: 0.0,
            scarcity: 1.0,
            samples: 6,
        }
    }
}
/// Optional biases from the seat's inclinations: `expansion` favours the roads strategy and
/// `development` the cities and cards strategies. Neutral at 0.5.
#[derive(Debug, Clone, Copy)]
pub struct Bias {
    pub expansion: f64,
    pub development: f64,
    pub weights: OpeningWeights,
}
impl Default for Bias {
    fn default() -> Self {
        Self {
            expansion: 0.5,
            development: 0.5,
            weights: OpeningWeights::default(),
        }
    }
}
impl Bias {
    fn factor(&self, strategy: &Strategy) -> f64 {
        let lean = |slider: f64| 1.0 + 0.15 * (2.0 * slider.clamp(0.0, 1.0) - 1.0);
        match strategy.name {
            "roads" => lean(self.expansion),
            "cities" | "cards" => lean(self.development),
            _ => 1.0,
        }
    }
}

fn pips(number: u8) -> f64 {
    if (2..=12).contains(&number) && number != 7 {
        f64::from(6 - number.abs_diff(7))
    } else {
        0.0
    }
}
/// Static facts about every intersection of a layout.
struct Sites {
    /// Pips per resource.
    production: [[f64; 5]; 54],
    /// Bit `n` set when a hex numbered `n` touches the intersection.
    numbers: [u16; 54],
    /// `Some(0)` generic 3:1, `Some(r + 1)` a 2:1 port for resource `r`.
    port: [Option<u8>; 54],
    /// Multiplier per resource: scarce resources on this island are worth more.
    scarcity: [f64; 5],
}
impl Sites {
    fn new(layout: &Layout, scarcity_strength: f64) -> Self {
        let t = topology();
        let mut production = [[0.0; 5]; 54];
        let mut numbers = [0u16; 54];
        let mut board = [0.0; 5];
        for h in 0..19u8 {
            let Some(r) = layout.resource(h) else {
                continue;
            };
            let n = layout.number(h);
            board[r as usize] += pips(n);
            for &v in &t.hex_vertices[h as usize] {
                production[v as usize][r as usize] += pips(n);
                numbers[v as usize] |= 1 << n;
            }
        }
        let mut port = [None; 54];
        for (i, vertices) in t.ports.iter().enumerate() {
            for &v in vertices {
                port[v as usize] = Some(layout.ports[i]);
            }
        }
        let mean = board.iter().sum::<f64>() / 5.0;
        let scarcity =
            board.map(|b| 1.0 + scarcity_strength * ((mean / b.max(1.0)).clamp(0.8, 1.5) - 1.0));
        Self {
            production,
            numbers,
            port,
            scarcity,
        }
    }
    fn total(&self, v: u8) -> f64 {
        self.production[v as usize].iter().sum()
    }
}
/// Occupancy the draft simulation edits: who owns each intersection and each edge.
#[derive(Clone, Copy)]
struct Occupancy {
    owner: [Option<u8>; 54],
    road: [Option<u8>; 72],
}
impl Occupancy {
    fn from_state(s: &State) -> Self {
        Self {
            owner: std::array::from_fn(|v| s.board.owner(v as u8)),
            road: std::array::from_fn(|e| s.board.road(e as u8)),
        }
    }
    fn open(&self, v: u8) -> bool {
        self.owner[v as usize].is_none()
            && topology().neighbors[v as usize]
                .iter()
                .all(|&n| self.owner[n as usize].is_none())
    }
    fn place(&mut self, v: u8, p: u8) {
        self.owner[v as usize] = Some(p);
    }
    fn first_settlement(&self, p: u8) -> Option<u8> {
        (0..54u8).find(|&v| self.owner[v as usize] == Some(p))
    }
}
/// Rolls until `cost` is affordable from `rates` (cards per roll), letting surplus convert at
/// each resource's ratio: the smallest horizon where the deficits fit inside the converted
/// surplus. Never more than 200 rolls.
pub fn fluid_eta(rates: &[f64; 5], cost: &Resources, ratios: &[f64; 5]) -> f64 {
    let feasible = |t: f64| {
        let mut deficit = 0.0;
        let mut credit = 0.0;
        for r in 0..5 {
            let have = rates[r] * t;
            let need = f64::from(cost.0[r]);
            if have < need {
                deficit += need - have;
            } else {
                credit += (have - need) / ratios[r];
            }
        }
        deficit <= credit + 1e-9
    };
    if feasible(0.0) {
        return 0.0;
    }
    let (mut low, mut high) = (0.0, 200.0);
    if !feasible(high) {
        return high;
    }
    for _ in 0..24 {
        let mid = (low + high) / 2.0;
        if feasible(mid) {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}
/// Reachable future sites from `v` for seat `p`: the best site's pips discounted by road
/// distance (two roads at least, since neighbours are illegal) and the number of open sites
/// two roads away. Foreign buildings and roads block the walk.
fn expansion(occ: &Occupancy, sites: &Sites, p: u8, v: u8) -> f64 {
    let t = topology();
    let mut distance = [u8::MAX; 54];
    let mut queue = vec![v];
    distance[v as usize] = 0;
    let mut cursor = 0;
    while cursor < queue.len() {
        let u = queue[cursor];
        cursor += 1;
        if distance[u as usize] >= 3 {
            continue;
        }
        for &e in &t.incident_edges[u as usize] {
            if occ.road[e as usize].is_some_and(|o| o != p) {
                continue;
            }
            let [a, b] = t.edges[e as usize];
            let next = if a == u { b } else { a };
            if occ.owner[next as usize].is_some_and(|o| o != p) {
                continue;
            }
            if distance[next as usize] == u8::MAX {
                distance[next as usize] = distance[u as usize] + 1;
                queue.push(next);
            }
        }
    }
    let mut best = 0.0f64;
    let mut near = 0.0f64;
    for u in 0..54u8 {
        let d = distance[u as usize];
        if u == v || !(2..=3).contains(&d) || !occ.open(u) {
            continue;
        }
        let mut value = sites.total(u);
        if sites.port[u as usize].is_some() {
            value += 1.0;
        }
        let discounted = value * if d == 2 { 1.0 } else { 0.55 };
        best = best.max(discounted);
        if d == 2 {
            near += 1.0;
        }
    }
    best + 0.7 * near.min(3.0)
}
/// Value of a pair (or a single site when `b` is `None`) under one strategy.
fn pair_terms(
    sites: &Sites,
    w: &OpeningWeights,
    strategy: &Strategy,
    a: u8,
    b: Option<u8>,
    expansion_a: f64,
    expansion_b: f64,
) -> Terms {
    let mut prod = sites.production[a as usize];
    let mut numbers = sites.numbers[a as usize];
    let mut ports = vec![sites.port[a as usize]];
    if let Some(b) = b {
        for (r, p) in prod.iter_mut().enumerate() {
            *p += sites.production[b as usize][r];
        }
        numbers |= sites.numbers[b as usize];
        ports.push(sites.port[b as usize]);
    }
    let production: f64 = (0..5)
        .map(|r| prod[r] * strategy.weights[r] * sites.scarcity[r])
        .sum::<f64>()
        * w.production;
    let coverage = (2..=12u8)
        .filter(|&n| numbers & (1 << n) != 0)
        .map(pips)
        .sum::<f64>()
        * w.coverage
        * strategy.coverage;
    // Two settlements can hold about three pips of every resource; below that a resource is
    // thin, and a missing one costs the full need.
    let scale = if b.is_some() { 1.0 } else { 0.5 };
    let balance = -(0..5)
        .map(|r| strategy.need[r] * (3.0 * scale - prod[r]).max(0.0))
        .sum::<f64>()
        * w.balance;
    let mut ratios = [4.0; 5];
    let mut port_value = 0.0;
    for port in ports.into_iter().flatten() {
        match port {
            0 => {
                for ratio in &mut ratios {
                    *ratio = f64::min(*ratio, 3.0);
                }
                port_value += 0.05 * prod.iter().sum::<f64>();
            }
            r => {
                let r = usize::from(r - 1);
                ratios[r] = 2.0;
                port_value += 0.3 * prod[r];
            }
        }
    }
    let ports = port_value * strategy.port * w.ports;
    let rates = prod.map(|p| p / 36.0);
    let mut settlement = Resources::SETTLEMENT;
    settlement.0[0] += 1;
    settlement.0[1] += 1;
    let turns = |cost: Resources| fluid_eta(&rates, &cost, &ratios) / 4.0;
    let tempo = (strategy.settlement_tempo * 5.0 / (1.0 + turns(settlement))
        + strategy.city_tempo * 7.0 / (1.0 + turns(Resources::CITY)))
        * w.tempo;
    let expansion = (expansion_a + expansion_b) * strategy.expansion * w.expansion;
    Terms {
        production,
        coverage,
        balance,
        tempo,
        ports,
        expansion,
        style: 0.0,
        draft: 0.0,
    }
}
/// The best strategy for a pair, with the bias applied as a `style` term so that the terms
/// still sum to the value.
fn best_pair(
    sites: &Sites,
    bias: &Bias,
    a: u8,
    b: Option<u8>,
    expansion_a: f64,
    expansion_b: f64,
) -> (f64, Terms, &'static str) {
    let w = &bias.weights;
    let mut base = pair_terms(sites, w, &STRATEGIES[0], a, b, expansion_a, expansion_b);
    base.style = base.total() * (bias.factor(&STRATEGIES[0]) - 1.0);
    let base_value = base.total();
    let mut best: Option<(f64, Terms, &'static str)> = None;
    for strategy in STRATEGIES.iter().skip(1) {
        let mut terms = pair_terms(sites, w, strategy, a, b, expansion_a, expansion_b);
        terms.style = terms.total() * (bias.factor(strategy) - 1.0);
        let value = terms.total();
        if value > base_value && best.as_ref().is_none_or(|(v, _, _)| value > *v) {
            best = Some((value, terms, strategy.name));
        }
    }
    match best {
        Some((value, terms, name)) if w.specialize > 0.0 => {
            // Blend the specialized terms toward the coverage terms by the knob.
            let k = w.specialize.clamp(0.0, 1.0);
            let mix = |x: f64, y: f64| y + k * (x - y);
            let blended = Terms {
                production: mix(terms.production, base.production),
                coverage: mix(terms.coverage, base.coverage),
                balance: mix(terms.balance, base.balance),
                tempo: mix(terms.tempo, base.tempo),
                ports: mix(terms.ports, base.ports),
                expansion: mix(terms.expansion, base.expansion),
                style: mix(terms.style, base.style),
                draft: 0.0,
            };
            let _ = value;
            let total = blended.total();
            (total, blended, name)
        }
        _ => (base_value, base, STRATEGIES[0].name),
    }
}
/// Snake order: the seat placing at `step` when `first` opened the draft.
fn setup_player(first: u8, players: u8, step: u8) -> u8 {
    let offset = if step < players {
        step
    } else {
        2 * players - 1 - step
    };
    (first + offset) % players
}
/// Which seat opened the draft, from the current seat and step (mirrors the engine's rule).
fn first_seat(s: &State) -> u8 {
    match s.phase {
        Phase::SetupSettlement { step } | Phase::SetupRoad { step, .. } => {
            let n = s.settings.players;
            let offset = if step < n { step } else { 2 * n - 1 - step };
            (s.current + n - offset) % n
        }
        _ => s.first,
    }
}
/// The greedy pick of seat `q` on an occupancy: the best single site for a first placement
/// or the best partner for its existing settlement. Returns the ranked open sites.
fn greedy(occ: &Occupancy, sites: &Sites, bias: &Bias, q: u8, cap: usize) -> Vec<(f64, u8)> {
    let first = occ.first_settlement(q);
    let expansion_first = first.map_or(0.0, |a| expansion(occ, sites, q, a));
    let mut scored: Vec<(f64, u8)> = (0..54u8)
        .filter(|&v| occ.open(v))
        .map(|v| {
            let e = expansion(occ, sites, q, v);
            let (value, _, _) = match first {
                Some(a) => best_pair(sites, bias, a, Some(v), expansion_first, e),
                None => best_pair(sites, bias, v, None, e, 0.0),
            };
            (value, v)
        })
        .collect();
    scored.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
    scored.truncate(cap);
    scored
}
fn draft_rng(salt: u32) -> Random {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&salt.to_le_bytes());
    bytes[4..8].copy_from_slice(b"open");
    Random::new(bytes)
}
/// Simulate the picks at `steps` on `occ`. Sample 0 is greedy; later samples pick among the
/// top few with weights falling off by the value gap, so the plan is robust to opponents
/// who rank sites a little differently.
fn simulate(
    occ: &mut Occupancy,
    sites: &Sites,
    first: u8,
    players: u8,
    steps: std::ops::Range<u8>,
    sample: u32,
) {
    let neutral = Bias::default();
    let mut rng = draft_rng(sample);
    for step in steps {
        let q = setup_player(first, players, step);
        let ranked = greedy(occ, sites, &neutral, q, 3);
        let Some(&(top, _)) = ranked.first() else {
            continue;
        };
        let pick = if sample == 0 {
            ranked[0].1
        } else {
            let weights: Vec<u32> = ranked
                .iter()
                .map(|(v, _)| ((1.0 + (top - v).max(0.0) / 2.0).recip() * 1000.0) as u32)
                .collect();
            let total: u32 = weights.iter().sum();
            let mut n = rng.below(total.max(1));
            let mut chosen = ranked[0].1;
            for (i, &w) in weights.iter().enumerate() {
                if n < w {
                    chosen = ranked[i].1;
                    break;
                }
                n -= w;
            }
            chosen
        };
        occ.place(pick, q);
    }
}
/// Rank the open sites for seat `p`'s settlement in the current setup step. A first
/// placement is valued by the pair it still leaves after the predicted intervening picks,
/// averaged over `samples` simulated drafts; a second placement by the pair it completes.
pub fn rank_settlements(s: &State, p: u8, samples: u32, bias: &Bias) -> Vec<Plan> {
    let samples = if samples == 0 {
        bias.weights.samples
    } else {
        samples
    };
    let Phase::SetupSettlement { step } = s.phase else {
        return vec![];
    };
    let sites = Sites::new(&s.layout, bias.weights.scarcity);
    let occ = Occupancy::from_state(s);
    let n = s.settings.players;
    let first = first_seat(s);
    if let Some(a) = occ.first_settlement(p) {
        // Second placement: complete the pair.
        let expansion_a = expansion(&occ, &sites, p, a);
        let mut plans: Vec<Plan> = (0..54u8)
            .filter(|&v| occ.open(v))
            .map(|v| {
                let e = expansion(&occ, &sites, p, v);
                let (value, terms, strategy) = best_pair(&sites, bias, a, Some(v), expansion_a, e);
                Plan {
                    vertex: v,
                    value,
                    terms,
                    strategy,
                    partner: Some(a),
                    survival: 1.0,
                }
            })
            .collect();
        plans.sort_by(|x, y| y.value.total_cmp(&x.value).then(x.vertex.cmp(&y.vertex)));
        return plans;
    }
    // Picks between this one and this seat's second placement.
    let next = (step + 1..2 * n)
        .find(|&t| setup_player(first, n, t) == p)
        .unwrap_or(2 * n);
    let between = step + 1..next;
    let shortlist = greedy(&occ, &sites, bias, p, 12);
    let samples = samples.max(1);
    let mut plans = Vec::with_capacity(shortlist.len());
    for (_, a) in shortlist {
        let mut placed = occ;
        placed.place(a, p);
        let expansion_a = expansion(&placed, &sites, p, a);
        // The pair this site would make with no draft in the way, and its terms.
        let (wish_value, wish_terms) = greedy(&placed, &sites, bias, p, 1)
            .first()
            .map(|&(_, b)| {
                let e = expansion(&placed, &sites, p, b);
                let (v, t, _) = best_pair(&sites, bias, a, Some(b), expansion_a, e);
                (v, t)
            })
            .unwrap_or_else(|| {
                let (v, t, _) = best_pair(&sites, bias, a, None, expansion_a, 0.0);
                (v, t)
            });
        let mut value = 0.0;
        let mut partners: BTreeMap<u8, u32> = BTreeMap::new();
        let mut strategies: BTreeMap<&'static str, u32> = BTreeMap::new();
        for sample in 0..samples {
            let mut world = placed;
            simulate(&mut world, &sites, first, n, between.clone(), sample);
            let mut best: Option<(f64, &'static str, u8)> = None;
            for b in (0..54u8).filter(|&v| world.open(v)) {
                let e = expansion(&world, &sites, p, b);
                let (v, _, name) = best_pair(&sites, bias, a, Some(b), expansion_a, e);
                if best.as_ref().is_none_or(|(bv, _, _)| v > *bv) {
                    best = Some((v, name, b));
                }
            }
            let (v, name, b) = best.unwrap_or_else(|| {
                let (v, _, name) = best_pair(&sites, bias, a, None, expansion_a, 0.0);
                (v, name, a)
            });
            value += v / f64::from(samples);
            *partners.entry(b).or_default() += 1;
            *strategies.entry(name).or_default() += 1;
        }
        let mut terms = wish_terms;
        terms.draft = (value - wish_value).min(0.0);
        let value = terms.total();
        let partner = partners.iter().max_by_key(|&(_, c)| *c).map(|(&b, _)| b);
        let survival = partners
            .get(&partner.unwrap_or(a))
            .map_or(0.0, |&c| f64::from(c) / f64::from(samples));
        let strategy = strategies
            .iter()
            .max_by_key(|&(_, c)| *c)
            .map_or("coverage", |(&n, _)| n);
        plans.push(Plan {
            vertex: a,
            value,
            terms,
            strategy,
            partner,
            survival,
        });
    }
    plans.sort_by(|x, y| y.value.total_cmp(&x.value).then(x.vertex.cmp(&y.vertex)));
    plans
}
/// Rank the free edges at the setup settlement `vertex`: each edge is worth the best open
/// site it leads to within two more roads, after the remaining setup picks are predicted,
/// plus a little for heading toward the seat's other settlement.
pub fn rank_roads(s: &State, p: u8, vertex: u8) -> Vec<(u8, f64, Option<u8>)> {
    let t = topology();
    let sites = Sites::new(&s.layout, OpeningWeights::default().scarcity);
    let mut occ = Occupancy::from_state(s);
    let n = s.settings.players;
    let first = first_seat(s);
    if let Phase::SetupRoad { step, .. } = s.phase {
        simulate(&mut occ, &sites, first, n, step + 1..2 * n, 0);
    }
    let other = (0..54u8).find(|&v| v != vertex && occ.owner[v as usize] == Some(p));
    let mut out = Vec::new();
    for &e in &t.incident_edges[vertex as usize] {
        if s.board.road(e).is_some() {
            continue;
        }
        let [a, b] = t.edges[e as usize];
        let u = if a == vertex { b } else { a };
        if occ.owner[u as usize].is_some_and(|o| o != p) {
            out.push((e, 0.0, None));
            continue;
        }
        let mut best: Option<(f64, u8)> = None;
        let mut near = 0.0f64;
        for &w in &t.neighbors[u as usize] {
            if w == vertex {
                continue;
            }
            let edge_uw = t.incident_edges[u as usize]
                .iter()
                .find(|&&f| t.edges[f as usize].contains(&w))
                .copied();
            if edge_uw.is_some_and(|f| s.board.road(f).is_some_and(|o| o != p)) {
                continue;
            }
            if occ.open(w) {
                near += 1.0;
                let value = (sites.total(w)
                    + if sites.port[w as usize].is_some() {
                        1.0
                    } else {
                        0.0
                    })
                    * 0.5;
                if best.is_none_or(|(v, _)| value > v) {
                    best = Some((value, w));
                }
            }
            if occ.owner[w as usize].is_some() {
                continue;
            }
            for &x in &t.neighbors[w as usize] {
                if x == u || !occ.open(x) {
                    continue;
                }
                let value = (sites.total(x)
                    + if sites.port[x as usize].is_some() {
                        1.0
                    } else {
                        0.0
                    })
                    * 0.28;
                if best.is_none_or(|(v, _)| value > v) {
                    best = Some((value, x));
                }
            }
        }
        let mut value = best.map_or(0.0, |(v, _)| v) + 0.35 * near.min(3.0);
        if let Some(o) = other
            && t.neighbors[u as usize].contains(&o)
        {
            value += 1.5;
        }
        if let Some(port) = sites.port[u as usize]
            && port == 0
        {
            value += 0.3;
        }
        out.push((e, value, best.map(|(_, w)| w)));
    }
    out.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
    out
}
/// The planner's action for seat `p` in a setup phase: the top-ranked settlement (greedy
/// draft prediction, one sample) or road. Used for the opponents in lookahead and as a
/// builder-grade default; `None` outside setup.
pub fn place(s: &State, p: u8) -> Option<Action> {
    match s.phase {
        Phase::SetupSettlement { .. } => {
            rank_settlements(s, p, 1, &Bias::default())
                .first()
                .map(|plan| Action::BuildSettlement {
                    vertex: plan.vertex,
                })
        }
        Phase::SetupRoad { vertex, .. } => rank_roads(s, p, vertex)
            .first()
            .map(|&(edge, _, _)| Action::BuildRoad { edge }),
        _ => None,
    }
}
/// A public-board state for the planner from a seated or spectator observation during
/// setup, so builders that see only an observation can use the same placements.
pub fn from_observation(o: &Observation) -> Option<State> {
    if !matches!(
        o.phase,
        Phase::SetupSettlement { .. } | Phase::SetupRoad { .. }
    ) {
        return None;
    }
    let board = Board::from_public(&o.buildings, &o.roads).ok()?;
    let n = o.settings.players;
    let step = match o.phase {
        Phase::SetupSettlement { step } | Phase::SetupRoad { step, .. } => step,
        _ => 0,
    };
    if step >= 2 * n || o.current >= n {
        return None;
    }
    let offset = if step < n { step } else { 2 * n - 1 - step };
    let first = (o.current + n - offset) % n;
    let mut s = State::new(o.settings, [0; 32]).ok()?;
    s.board = board;
    s.layout = Layout {
        hexes: o.hexes,
        ports: o.ports,
    };
    s.robber = o.robber;
    s.phase = o.phase;
    s.current = o.current;
    s.first = first;
    s.turn = o.turn;
    Some(s)
}
/// A short public explanation of a plan for reports and table talk.
pub fn describe(plan: &Plan, layout: &Layout) -> String {
    let sites = Sites::new(layout, 0.0);
    let prod = sites.production[plan.vertex as usize];
    let names = ["brick", "lumber", "wool", "grain", "ore"];
    let mut parts: Vec<String> = (0..5)
        .filter(|&r| prod[r] > 0.0)
        .map(|r| format!("{} {}", prod[r] as u32, names[r]))
        .collect();
    if parts.is_empty() {
        parts.push("no production".into());
    }
    let mut text = format!(
        "{} portfolio: site {} gives {} pips",
        plan.strategy,
        plan.vertex,
        parts.join(", ")
    );
    if let Some(b) = plan.partner
        && b != plan.vertex
    {
        if plan.survival < 1.0 {
            text += &format!(
                "; expects to pair it with site {} ({}% of predicted drafts)",
                b,
                (plan.survival * 100.0).round() as u32
            );
        } else {
            text += &format!("; pairs with site {b}");
        }
    }
    if plan.terms.draft < -0.5 {
        text += &format!(
            "; the draft is expected to cost {:.1} pips",
            -plan.terms.draft
        );
    }
    text
}
