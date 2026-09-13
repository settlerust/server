//! `expectimax-v2`: a tunable turn-level expectimax with iterative deepening. See
//! `docs/expectimax.md` for the exact semantics of every setting.
pub mod bargain;
pub mod key;
pub mod leaf;
pub mod opening;
pub mod opponent;
pub mod predict;
pub mod search;
pub mod weights;
use crate::search::{
    Candidate, Contributions, OutcomeLevel, OutcomeSummary, Report, SearchError, Statistics,
    TradeAnalysis,
};
use crate::{sampling, trades};
pub use search::Cache;
use search::{Limits, Search, World};
use serde::{Deserialize, Serialize};
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{EventBatch, GameView, OfferStatus};
use std::collections::BTreeMap;
use std::sync::Arc;
pub use weights::{Inclinations, Weights};

/// Which leaf the search uses. Without `tables` the hand-written terms apply. With a table
/// path, the learned n-tuple value (win-probability units) is multiplied by `scale` so that
/// it lives on the hand-written currency; `hand` adds the hand-written terms scaled by that
/// factor (zero means the tables alone). Tables load once per process and are shared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LeafConfig {
    pub tables: Option<String>,
    pub scale: f64,
    pub hand: f64,
}
impl Default for LeafConfig {
    fn default() -> Self {
        Self {
            tables: None,
            scale: 1000.0,
            hand: 0.0,
        }
    }
}
/// The leaf evaluator handed to a search: hand-written terms, or learned tables.
#[derive(Clone)]
pub enum Leaf {
    Hand,
    Tables {
        model: Arc<crate::ntuple::Model>,
        scale: f64,
        hand: f64,
    },
}
impl Leaf {
    /// Frozen tables from memory, with the given scale and hand-written blend.
    pub fn tables(model: Arc<crate::ntuple::Model>, scale: f64, hand: f64) -> Self {
        Self::Tables { model, scale, hand }
    }
    /// Resolve a configuration: load the named tables (native builds only).
    pub fn from_config(config: &SearchConfig) -> Result<Self, SearchError> {
        let Some(path) = &config.leaf.tables else {
            return Ok(Self::Hand);
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(SearchError::Tables(
                "learned tables are not available in the browser build".into(),
            ))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let model = crate::ntuple::shared(path).map_err(SearchError::Tables)?;
            Ok(Self::Tables {
                model,
                scale: config.leaf.scale,
                hand: config.leaf.hand,
            })
        }
    }
    pub fn is_hand(&self) -> bool {
        matches!(self, Self::Hand)
    }
    /// Static value of a position from seat `p`'s perspective. A finished game is exact in
    /// both leaves; the tables never see terminal positions. `knight_play_value` shapes the
    /// hand-written development term, including the blend under a tables leaf.
    #[inline]
    pub fn evaluate(&self, s: &State, p: u8, w: &Weights, knight_play_value: bool) -> leaf::Terms {
        match self {
            Self::Hand => leaf::evaluate_with(s, p, w, knight_play_value),
            Self::Tables { model, scale, hand } => {
                let mut terms = leaf::Terms::default();
                if let Phase::Finished { winner } = s.phase {
                    terms
                        .0
                        .insert("terminal", if winner == p { *scale } else { 0.0 });
                    return terms;
                }
                terms
                    .0
                    .insert("tables", f64::from(model.value(s, p)) * scale);
                if *hand > 0.0 {
                    terms.add_scaled(&leaf::evaluate_with(s, p, w, knight_play_value), *hand);
                }
                terms
            }
        }
    }
}

/// Search settings. Depth is the cap on planned own turns; time and node budgets stop
/// iterative deepening early, and the deepest completed depth is played.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    pub depth: u8,
    /// Wall-clock limit in milliseconds; zero disables the clock and leaves only the node budget.
    pub time_budget_ms: f64,
    /// Engine transitions allowed per analysis before the current depth is abandoned.
    pub node_budget: u64,
    pub samples: u8,
    /// Common random scenarios for the opponents' round after the current turn.
    pub scenarios: u8,
    /// Scenarios at deeper levels.
    pub inner_scenarios: u8,
    /// First actions expanded per inner turn.
    pub beam: u8,
    /// Distinct end-of-turn positions enumerated per planned turn.
    pub afterstates: u16,
    pub iterative: bool,
    pub weights: Weights,
    pub inclinations: Inclinations,
    /// Ablation switches. `builder_setup` places the opening with the builder heuristic
    /// instead of the search; `counts_only` ignores event history and samples hands from
    /// public counts alone; `leaf_opponents` makes opponents pick the action that maximizes
    /// their own static leaf instead of following the builder; `propose` enables trade offers.
    pub builder_setup: bool,
    pub counts_only: bool,
    pub leaf_opponents: bool,
    pub propose: bool,
    /// Scale the willingness model per partner by that partner's observed acceptance rate of
    /// this seat's offers (Laplace-smoothed from the public trade record).
    pub reciprocity: bool,
    /// Leaf evaluator: hand-written terms by default, or learned n-tuple tables.
    pub leaf: LeafConfig,
    /// Iterative deepening continues only while the elapsed time is below this fraction of
    /// the budget, because the next depth costs several times more.
    pub deepen_fraction: f64,
    /// Decision-aware deepening: once the best candidate leads the runner-up by at least this
    /// many leaf units the decision is treated as settled and deepening stops. Zero disables
    /// the rule. A position with a single candidate never deepens past the first depth.
    pub deepen_gap: f64,
    /// Explain mode: report the distribution of end-of-turn positions the chosen action leads
    /// to at every planned level, with probabilities. Costs memory, never changes the decision.
    pub explain: bool,
    /// Transposition tables: `none` rebuilds them for every depth pass (the original search),
    /// `decision` shares them across the depth passes of one decision, and `turn` keeps them
    /// across every decision of the same turn. `decision` reproduces `none` bit for bit
    /// whenever no node or time budget binds, and completes more of the tree when one does;
    /// `turn` also reuses values computed under the earlier decisions' sampled hands.
    pub cache: CacheMode,
    /// How the two setup settlements and their roads are placed: `planner` ranks portfolios
    /// of the two sites under discrete strategies with the draft predicted (the default);
    /// `search` keeps the planner's shortlist but lets the lookahead rank it; `builder_setup`
    /// above overrides both with the plain builder heuristic.
    pub opening: OpeningMode,
    /// Term weights of the opening planner.
    pub opening_weights: opening::OpeningWeights,
    /// Threat-aware bargaining: partner threats price every swap, open offers draw counters,
    /// and well-stocked partners are asked two for one. `false` is the all-off control: plain
    /// acceptance and proposals with the reference willingness model, no threat costs, no
    /// counters, and no asks.
    pub bargain: bool,
    /// One-change arms of the bargaining, each on by default and effective only while
    /// `bargain` is on: `threat_pricing` applies the threat premium on the trade margin and
    /// the partner cost, `counters` answers open offers with counters, and `asks` proposes
    /// two-for-one asks to partners known to hold plenty.
    pub threat_pricing: bool,
    pub counters: bool,
    pub asks: bool,
    /// Whether this seat may accept an offer or counter that asks it for more cards than it
    /// receives. `false` stands in for a stubborn partner who refuses every ask.
    pub accept_asks: bool,
    /// Build prediction switches; see `PredictConfig`.
    pub predict: PredictConfig,
    /// Endgame switches: a race leaf against a near-winning leader, expected hidden victory
    /// points, and late robber and road bias toward the leader. See `EndgameConfig`.
    pub endgame: weights::EndgameConfig,
    /// Robber candidate scoring switches; see `RobberConfig`.
    pub robber: RobberConfig,
    /// Development-card timing mechanisms, one switch per hypothesis, all off by default.
    /// `knight_play_value` values played knights in the hand-written leaf by largest-army
    /// distance (two away, one away, taking it) instead of the flat `knight` weight, and
    /// credits a held playable knight with one roll of the production it would unblock.
    /// `buy_tempo` offers a development purchase only when no road, settlement, or city is
    /// affordable within the next own turn at current production. `hold_knight` offers
    /// main-phase knight plays only when the robber sits on this seat's best hex, so the
    /// knight is held to be played before the roll; the roll phase keeps the choice.
    pub knight_play_value: bool,
    pub buy_tempo: bool,
    pub hold_knight: bool,
    /// When the hand exceeds the discard limit, also consider bank trades and roads chosen
    /// only to shed cards before another seat's roll. `false` (the default) considers only
    /// builds, useful roads, and trades that reduce a build deficit; the leaf's `hand_risk`
    /// term prices holding either way.
    pub spend_down: bool,
    /// Score discard choices by the build deficits of the kept hand plus the expected
    /// production before the next own turn, instead of the kept hand alone. `false` (the
    /// default) ranks discards by the current hand's deficits.
    pub discard_projection: bool,
}
/// Build prediction switches. Each is off by default, reproducing the search exactly.
/// `opponents` makes the lookahead's opponent model execute each opponent's predicted next
/// build instead of the builder's greedy step-by-step policy; `threat` makes the bargaining
/// `space` threat key on the other seat's predicted target site instead of scanning this
/// seat's best three sites for reachability. The denial leaf term is the `denial` weight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PredictConfig {
    pub opponents: bool,
    pub threat: bool,
}
/// See `SearchConfig::opening`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpeningMode {
    #[default]
    Planner,
    Search,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    #[default]
    None,
    Decision,
    Turn,
}
/// Robber candidate scoring. All switches off is the original behaviour: blocked opponent pips
/// weighted by the blocked seat's public points, plus a victim bonus for hand size and points.
/// The score only orders the shortlist of placements the lookahead then values; it is not
/// the value the search assigns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RobberConfig {
    /// Weight the hex score toward the points leader: the leader's blocked pips count in
    /// full, every other seat's at a tenth.
    pub leader: bool,
    /// Choose the victim by the expected share of the resource this seat most needs, from
    /// its resource knowledge, instead of by hand size.
    pub need: bool,
    /// Weight each seat's blocked pips by their threat, and never steal from a seat whose
    /// threat is below 0.5 while any placement without such a victim exists.
    pub threat: bool,
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            depth: 2,
            time_budget_ms: 0.0,
            node_budget: 400_000,
            samples: 4,
            scenarios: 8,
            inner_scenarios: 4,
            beam: 4,
            afterstates: 96,
            iterative: true,
            weights: Weights::default(),
            inclinations: Inclinations::default(),
            builder_setup: false,
            counts_only: false,
            leaf_opponents: false,
            propose: true,
            reciprocity: false,
            leaf: LeafConfig::default(),
            deepen_fraction: 0.3,
            deepen_gap: 0.0,
            explain: false,
            cache: CacheMode::None,
            opening: OpeningMode::Planner,
            opening_weights: opening::OpeningWeights::default(),
            bargain: true,
            threat_pricing: true,
            counters: true,
            asks: true,
            accept_asks: true,
            predict: PredictConfig::default(),
            endgame: weights::EndgameConfig::default(),
            robber: RobberConfig::default(),
            knight_play_value: false,
            buy_tempo: false,
            hold_knight: false,
            spend_down: false,
            discard_projection: false,
        }
    }
}
impl SearchConfig {
    pub fn bounded(mut self) -> Self {
        self.depth = self.depth.clamp(1, 6);
        self.node_budget = self.node_budget.clamp(1_000, 200_000_000);
        self.samples = self.samples.clamp(1, 32);
        self.scenarios = self.scenarios.clamp(1, 64);
        self.inner_scenarios = self.inner_scenarios.clamp(1, 32);
        self.beam = self.beam.clamp(1, 16);
        self.afterstates = self.afterstates.clamp(4, 1024);
        self.time_budget_ms = self.time_budget_ms.clamp(0.0, 3_600_000.0);
        self.inclinations = self.inclinations.clamped();
        self.leaf.scale = self.leaf.scale.clamp(1.0, 1_000_000.0);
        self.leaf.hand = self.leaf.hand.clamp(0.0, 10.0);
        self.deepen_fraction = self.deepen_fraction.clamp(0.01, 1.0);
        self.deepen_gap = self.deepen_gap.clamp(0.0, 1_000_000.0);
        self.endgame.margin = self.endgame.margin.clamp(0.0, 10.0);
        self.endgame.tempo = self.endgame.tempo.clamp(0.0, 10.0);
        self.endgame.trigger = self.endgame.trigger.clamp(0, 8);
        self.endgame.block_from = self.endgame.block_from.clamp(0.0, 1.0);
        self.endgame.block_bias = self.endgame.block_bias.clamp(0.0, 10.0);
        self
    }
    fn limits(&self, depth: u8) -> Limits {
        Limits {
            depth,
            beam: usize::from(self.beam),
            root_beam: usize::from(self.beam),
            per_candidate: 1,
            afterstates: usize::from(self.afterstates),
            scenarios: usize::from(self.scenarios),
            inner_scenarios: usize::from(self.inner_scenarios),
            node_budget: self.node_budget,
            leaf_opponents: self.leaf_opponents,
            predict_opponents: self.predict.opponents,
            opening: if self.builder_setup {
                OpeningMode::Search
            } else {
                self.opening
            },
            robber: self.robber,
            expected_hands: None,
            predict_threat: self.predict.threat,
            knight_play_value: self.knight_play_value,
            buy_tempo: self.buy_tempo,
            hold_knight: self.hold_knight,
            spend_down: self.spend_down,
            discard_projection: self.discard_projection,
        }
    }
}
struct RootValue {
    value: leaf::Terms,
    trade: Option<TradeAnalysis>,
}
/// One root candidate: its command (none for waiting), the action it stands for, its value,
/// and the explanation and table talk that go with it.
struct Row {
    command: Option<settlers_protocol::Command>,
    action: Action,
    root: RootValue,
    note: Option<String>,
    speech: Option<String>,
}
impl Row {
    fn plain(command: Option<settlers_protocol::Command>, action: Action, root: RootValue) -> Self {
        Self {
            command,
            action,
            root,
            note: None,
            speech: None,
        }
    }
}
/// A completed depth: rows, statistics, and the recorded outcomes per plain action.
type Completed = (u8, Vec<Row>, search::Stats, Vec<Vec<search::Outcome>>);
fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}

/// Rank the root player's decisions using only the seated observation and its own history.
/// `clock` returns elapsed milliseconds since the call began. The leaf named by the
/// configuration is resolved here; use `analyze_with` to pass tables already in memory.
pub fn analyze(
    view: &GameView,
    history: &[EventBatch],
    config: &SearchConfig,
    offered_turn: Option<u32>,
    clock: &dyn Fn() -> f64,
) -> Result<Report, SearchError> {
    let leaf = Leaf::from_config(config)?;
    let mut cache = Cache::default();
    analyze_with(
        view,
        history,
        config,
        offered_turn,
        clock,
        &leaf,
        &mut cache,
    )
}
/// `analyze` with an explicit leaf evaluator and the caller's transposition tables. The
/// tables are cleared according to `config.cache` before the search runs.
pub fn analyze_with(
    view: &GameView,
    history: &[EventBatch],
    config: &SearchConfig,
    offered_turn: Option<u32>,
    clock: &dyn Fn() -> f64,
    leaf: &Leaf,
    cache: &mut Cache,
) -> Result<Report, SearchError> {
    if view.research_hands.is_some() {
        return Err(SearchError::NotSeated);
    }
    let o = view.game.as_ref().ok_or(SearchError::NotSeated)?;
    let seat = o.seat.ok_or(SearchError::NotSeated)?;
    let config = config.clone().bounded();
    let empty: Vec<EventBatch> = Vec::new();
    let history = if config.counts_only {
        &empty[..]
    } else {
        history
    };
    let mut knowledge = ResourceKnowledge::from_history_at_version(o, history, view.version);
    for offer in &view.offers {
        if offer.status == OfferStatus::Open && offer.from < o.settings.players {
            knowledge.players[offer.from as usize].condition_at_least(offer.give);
        }
    }
    knowledge.condition_on_bank(o);
    let states = sampling::worlds(o, &knowledge, config.samples)?;
    let worlds: Vec<World> = states
        .iter()
        .map(|&state| World {
            state,
            weight: 1.0 / states.len() as f64,
        })
        .collect();
    let mut weights = config.inclinations.apply(config.weights);
    weights.endgame = config.endgame;
    // Expected hidden-hand composition per seat, for the need-aware robber victim bonus.
    let expected_hands: Option<[[f64; 5]; 4]> = config.robber.need.then(|| {
        let mut table = [[0.0; 5]; 4];
        for (entry, estimate) in table.iter_mut().zip(&knowledge.players) {
            *entry = estimate.expected;
        }
        table
    });
    let limits_of = |depth: u8| {
        let mut limits = config.limits(depth);
        limits.expected_hands = expected_hands;
        limits
    };
    let partner_factor = if config.reciprocity {
        reciprocity(history, seat)
    } else {
        [1.0; 4]
    };
    let margin = config.inclinations.trade_margin();
    let root = &worlds[0].state;
    let my_turn = actor(root) == seat;
    // Who is dangerous to trade with, and the site the top threat is heading for.
    let threats = bargain::threats(root, seat, config.predict.threat);
    let top_threat = bargain::top_threat(&threats, o.settings.players);
    let contested = top_threat.and_then(|(q, _)| bargain::contested_site(root, seat, q));
    let aggression = 0.5 + config.inclinations.clamped().aggression;
    // The one-change arms are only effective while bargaining is on; `bargain: false` turns
    // every one of them off.
    let counters = config.bargain && config.counters;
    let asks = config.bargain && config.asks;
    let threat_pricing = config.bargain && config.threat_pricing;
    let mut decisions = bargain::responses(view, &knowledge, history);
    decisions.retain(|d| match &d.kind {
        bargain::Kind::Counter { .. } => counters,
        // A stubborn seat never accepts terms that ask it for more cards than it receives.
        bargain::Kind::Accept { offer } => {
            config.accept_asks || offer.receive.total() <= offer.give.total()
        }
        _ => true,
    });
    if config.propose {
        decisions.extend(bargain::proposals(
            view,
            &knowledge,
            &threats,
            0.75,
            history,
            offered_turn,
        ));
        if !asks {
            decisions.retain(|d| !matches!(d.kind, bargain::Kind::Offer { ask: true, .. }));
        }
    }
    // Margin and cost of dealing with a seat: the plain margin plus a threat premium, and a
    // share of what the partner gains scaled by their threat and this seat's aggression.
    let threat_scale = if threat_pricing { 1.0 } else { 0.0 };
    let margin_for = |q: u8| margin + threats[q as usize].total * 30.0 * threat_scale;
    let threat_cost =
        |q: u8, gain: f64| threats[q as usize].total * aggression * 0.5 * gain * threat_scale;
    let card_value = weights.card_value;
    let point = weights.point;
    match config.cache {
        CacheMode::None | CacheMode::Decision => cache.clear(),
        CacheMode::Turn => {
            if cache.owner != Some((root.turn, root.current)) {
                cache.clear();
                cache.owner = Some((root.turn, root.current));
            }
        }
    }
    // Root actions come from the v2 generator; trade proposals and acceptances from the shared
    // enumerator. Discards get several candidates instead of the single automatic suggestion.
    let mut actions: Vec<Action> = Vec::new();
    if my_turn {
        let mut probe_cache = Cache::default();
        let probe = Search::new(
            seat,
            weights,
            limits_of(1),
            &|| false,
            leaf.clone(),
            &mut probe_cache,
        );
        actions = if matches!(root.phase, Phase::Discard { .. }) {
            Search::discard_candidates(root, seat, 4, config.discard_projection)
        } else {
            probe.candidates(root, seat, 0)
        };
        actions.retain(|a| {
            o.legal_actions.contains(a)
                || matches!(
                    a,
                    Action::Discard { .. }
                        | Action::BankTrade { .. }
                        | Action::PlayYearOfPlenty { .. }
                )
        });
    }
    // Setup placements come straight from the opening planner: the lookahead through the
    // first rolls adds noise, not information, and the planner already predicts the draft.
    let in_setup = matches!(
        root.phase,
        Phase::SetupSettlement { .. } | Phase::SetupRoad { .. }
    );
    if my_turn && in_setup && config.opening == OpeningMode::Planner && !config.builder_setup {
        let bias = opening::Bias {
            expansion: config.inclinations.expansion,
            development: config.inclinations.development,
            weights: config.opening_weights,
        };
        let mut candidates: Vec<Candidate> = Vec::new();
        match root.phase {
            Phase::SetupSettlement { .. } => {
                for plan in opening::rank_settlements(root, seat, 0, &bias) {
                    let action = Action::BuildSettlement {
                        vertex: plan.vertex,
                    };
                    if !o.legal_actions.contains(&action) {
                        continue;
                    }
                    let mut line = vec![action];
                    if let Some(partner) = plan.partner
                        && partner != plan.vertex
                        && root.board.owner(partner).is_none()
                    {
                        line.push(Action::BuildSettlement { vertex: partner });
                    }
                    candidates.push(Candidate {
                        id: String::new(),
                        command: Some(settlers_protocol::Command::Act { action }),
                        expected_value: plan.value,
                        minimum_leaf_value: plan.value,
                        maximum_leaf_value: plan.value,
                        contributions: Contributions(
                            plan.terms
                                .named()
                                .into_iter()
                                .map(|(k, v)| (k.to_string(), v))
                                .collect(),
                        ),
                        trade: None,
                        principal_variation: line,
                        note: Some(opening::describe(&plan, &root.layout)),
                        speech: None,
                    });
                }
            }
            Phase::SetupRoad { vertex, .. } => {
                for (edge, value, target) in opening::rank_roads(root, seat, vertex) {
                    let action = Action::BuildRoad { edge };
                    if !o.legal_actions.contains(&action) {
                        continue;
                    }
                    candidates.push(Candidate {
                        id: String::new(),
                        command: Some(settlers_protocol::Command::Act { action }),
                        expected_value: value,
                        minimum_leaf_value: value,
                        maximum_leaf_value: value,
                        contributions: Contributions(BTreeMap::from([(
                            "expansion".to_string(),
                            value,
                        )])),
                        trade: None,
                        principal_variation: vec![action],
                        note: target.map(|w| format!("road toward site {w}")),
                        speech: None,
                    });
                }
            }
            _ => {}
        }
        if !candidates.is_empty() {
            return Ok(finish_report(
                &config,
                view,
                o,
                seat,
                leaf,
                knowledge,
                candidates,
                search::Stats::default(),
                0,
                actions.len(),
                cache,
                clock(),
                None,
            ));
        }
    }
    let deadline_ms = config.time_budget_ms;
    let over_time = || deadline_ms > 0.0 && clock() >= deadline_ms;
    let mut best_report: Option<Completed> = None;
    let max_depth = config.depth;
    let start_depth = if config.iterative { 1 } else { config.depth };
    let mut cumulative = search::Stats::default();
    for depth in start_depth..=max_depth {
        if config.cache == CacheMode::None {
            cache.clear();
        }
        let mut search = Search::new(
            seat,
            weights,
            limits_of(depth),
            &over_time,
            leaf.clone(),
            cache,
        );
        search.record = config.explain;
        let mut rows: Vec<Row> = Vec::new();
        let mut recorded: Vec<Vec<search::Outcome>> = Vec::new();
        let mut best_plain: Option<f64> = None;
        for a in &actions {
            let value = root_action(&mut search, &worlds, seat, *a, depth)?;
            recorded.push(std::mem::take(&mut search.outcomes));
            if search.aborted {
                break;
            }
            best_plain = Some(best_plain.map_or(value.total(), |b: f64| b.max(value.total())));
            rows.push(Row::plain(
                Some(settlers_protocol::Command::Act { action: *a }),
                *a,
                RootValue { value, trade: None },
            ));
        }
        // The value of answering nothing: the opponents' round from here (off turn only).
        let wait_value = if !search.aborted && !my_turn && !decisions.is_empty() {
            Some(search.round(&worlds, 0)?)
        } else {
            None
        };
        let fallback_plain = best_plain.or(wait_value.as_ref().map(|v| v.total()));
        // Accept values per offer id, for the counters' fallback and the decline reasons.
        let mut accept_totals: BTreeMap<String, f64> = BTreeMap::new();
        let mut counter_best: BTreeMap<String, f64> = BTreeMap::new();
        // Per-partner proposal analyses, for the combined offers to every safe partner.
        let mut per_partner: Vec<PartnerOffer> = Vec::new();
        if !search.aborted {
            for d in &decisions {
                if search.aborted {
                    break;
                }
                let Action::Trade {
                    other: partner,
                    give,
                    receive,
                } = d.action
                else {
                    continue;
                };
                match &d.kind {
                    bargain::Kind::Offer { partner: _, ask } => {
                        let Some(best_plain) = best_plain else {
                            continue;
                        };
                        let mut accepted = Vec::new();
                        let mut affordable = 0.0;
                        let mut unlocked = Vec::new();
                        let mut gain = 0.0;
                        for w in &worlds {
                            let prob = trades::acceptance(&w.state, seat, d.action)
                                * (0.5 + config.inclinations.trade)
                                * partner_factor[partner as usize]
                                * if *ask { 0.85 } else { 1.0 };
                            if let Ok(after) = apply(&w.state, seat, d.action) {
                                affordable += w.weight;
                                for a in trades::unlocked(&w.state, &after, seat) {
                                    if !unlocked.contains(&a) {
                                        unlocked.push(a);
                                    }
                                }
                                gain += w.weight
                                    * bargain::partner_gain(
                                        &w.state, partner, give, receive, card_value, point,
                                    );
                                accepted.push(World {
                                    state: after,
                                    weight: w.weight * prob.min(1.0),
                                });
                            }
                        }
                        let probability: f64 = accepted.iter().map(|w| w.weight).sum();
                        if probability <= 1e-9 {
                            continue;
                        }
                        if affordable > 0.0 {
                            gain /= affordable;
                        }
                        let accepted_value = search.plan(accepted, 1)?.0;
                        if search.aborted {
                            break;
                        }
                        let accepted_total = accepted_value.total();
                        let cost = threat_cost(partner, gain);
                        let mut value = leaf::Terms::default();
                        value.0.insert(
                            "expected",
                            best_plain + probability * (accepted_total - best_plain),
                        );
                        value.0.insert("trade_margin", -margin_for(partner));
                        value.0.insert("partner_threat", -probability * cost);
                        per_partner.push((
                            give,
                            receive,
                            partner,
                            probability,
                            accepted_total,
                            cost,
                            unlocked.clone(),
                        ));
                        let block = blocks(&unlocked, contested)
                            .then_some(top_threat.map(|t| t.0))
                            .flatten();
                        let text = bargain::offer_text(
                            view,
                            &[partner],
                            give,
                            receive,
                            &unlocked,
                            block,
                            *ask,
                        );
                        let command = settlers_protocol::Command::OfferTrade {
                            to: vec![partner],
                            give,
                            receive,
                            expires_in_ms: None,
                            message: Some(settlers_protocol::Communication {
                                intent: settlers_protocol::Intent::RequestTrade,
                                resources: Some(receive),
                                text: Some(text),
                            }),
                        };
                        rows.push(Row {
                            command: Some(command),
                            action: d.action,
                            root: RootValue {
                                value,
                                trade: Some(TradeAnalysis {
                                    acceptance_probability: probability,
                                    acceptance_when_affordable: if affordable > 0.0 {
                                        probability / affordable
                                    } else {
                                        0.0
                                    },
                                    accepted_value: Some(accepted_total),
                                    rejected_value: best_plain,
                                    partner_affordability: affordable,
                                    unlocked_actions: unlocked,
                                }),
                            },
                            note: Some(format!(
                                "{}: threat {:.2}, modeled acceptance {:.2}, partner gain {:.0}",
                                bargain::name(view, partner),
                                threats[partner as usize].total,
                                probability,
                                gain
                            )),
                            speech: None,
                        });
                    }
                    bargain::Kind::Accept { offer } => {
                        // Only worlds where the partner can pay.
                        let feasible: Vec<World> = worlds
                            .iter()
                            .filter_map(|w| {
                                apply(&w.state, seat, d.action).ok().map(|state| World {
                                    state,
                                    weight: w.weight,
                                })
                            })
                            .collect();
                        if feasible.is_empty() {
                            continue;
                        }
                        // The partner gets what this seat gives and pays what it receives.
                        let gain = worlds
                            .iter()
                            .map(|w| {
                                w.weight
                                    * bargain::partner_gain(
                                        &w.state, partner, give, receive, card_value, point,
                                    )
                            })
                            .sum::<f64>();
                        let mut value = if actor(&feasible[0].state) == seat
                            && feasible[0].state.phase == Phase::Main
                        {
                            search.plan(feasible, 1)?.0
                        } else {
                            search.round(&normalize(feasible), 0)?
                        };
                        value.0.insert("trade_margin", -margin_for(partner));
                        value
                            .0
                            .insert("partner_threat", -threat_cost(partner, gain));
                        accept_totals.insert(offer.id.clone(), value.total());
                        rows.push(Row {
                            command: Some(d.command.clone()),
                            action: d.action,
                            root: RootValue { value, trade: None },
                            note: Some(format!(
                                "{}: threat {:.2}, partner gain {:.0}",
                                bargain::name(view, partner),
                                threats[partner as usize].total,
                                gain
                            )),
                            speech: Some(bargain::accept_text(view, offer)),
                        });
                    }
                    bargain::Kind::Counter { offer, acceptance } => {
                        let Some(fallback_plain) = fallback_plain else {
                            continue;
                        };
                        let mut accepted = Vec::new();
                        let mut gain = 0.0;
                        let mut affordable = 0.0;
                        for w in &worlds {
                            if let Ok(after) = apply(&w.state, seat, d.action) {
                                affordable += w.weight;
                                gain += w.weight
                                    * bargain::partner_gain(
                                        &w.state, partner, give, receive, card_value, point,
                                    );
                                accepted.push(World {
                                    state: after,
                                    weight: w.weight,
                                });
                            }
                        }
                        if accepted.is_empty() {
                            continue;
                        }
                        gain /= affordable;
                        let probability =
                            (acceptance * partner_factor[partner as usize] * affordable).min(1.0);
                        let accepted_value = search.round(&normalize(accepted), 0)?;
                        if search.aborted {
                            break;
                        }
                        let accepted_total = accepted_value.total() - threat_cost(partner, gain);
                        // If the counter is declined the original offer usually stays open.
                        let original = accept_totals.get(&offer.id).copied();
                        let fallback = match original {
                            Some(accept) => {
                                fallback_plain + 0.6 * (accept - fallback_plain).max(0.0)
                            }
                            None => fallback_plain,
                        };
                        let mut value = leaf::Terms::default();
                        value.0.insert(
                            "expected",
                            probability * accepted_total + (1.0 - probability) * fallback,
                        );
                        value.0.insert("trade_margin", -margin_for(partner));
                        let total = value.total();
                        counter_best
                            .entry(offer.id.clone())
                            .and_modify(|b| *b = b.max(total))
                            .or_insert(total);
                        rows.push(Row {
                            command: Some(d.command.clone()),
                            action: d.action,
                            root: RootValue {
                                value,
                                trade: Some(TradeAnalysis {
                                    acceptance_probability: probability,
                                    acceptance_when_affordable: *acceptance,
                                    accepted_value: Some(accepted_total),
                                    rejected_value: fallback,
                                    partner_affordability: affordable,
                                    unlocked_actions: vec![],
                                }),
                            },
                            note: Some(format!(
                                "counter to {}: estimated eagerness {:.2}, threat {:.2}",
                                bargain::name(view, partner),
                                acceptance,
                                threats[partner as usize].total
                            )),
                            speech: None,
                        });
                    }
                    bargain::Kind::Reject { .. } => {}
                }
            }
        }
        // One offer to every safe partner at once, when several were worth asking.
        if !search.aborted
            && let Some(best_plain) = best_plain
        {
            let mut done: Vec<(Resources, Resources)> = Vec::new();
            for &(give, receive, _, _, _, _, _) in &per_partner {
                if done.contains(&(give, receive)) {
                    continue;
                }
                done.push((give, receive));
                let group: Vec<_> = per_partner
                    .iter()
                    .filter(|x| x.0 == give && x.1 == receive && threats[x.2 as usize].total < 0.5)
                    .collect();
                if group.len() < 2 {
                    continue;
                }
                let miss: f64 = group.iter().map(|x| 1.0 - x.3).product();
                let probability = 1.0 - miss;
                let mass: f64 = group.iter().map(|x| x.3).sum();
                if probability <= 1e-9 || mass <= 1e-9 {
                    continue;
                }
                let accepted_total: f64 = group.iter().map(|x| x.3 * x.4).sum::<f64>() / mass;
                let cost: f64 = group.iter().map(|x| x.3 * x.5).sum::<f64>() / mass;
                let to: Vec<u8> = group.iter().map(|x| x.2).collect();
                let margin_group = group.iter().map(|x| margin_for(x.2)).fold(0.0, f64::max);
                let mut unlocked: Vec<Action> = Vec::new();
                for x in &group {
                    for a in &x.6 {
                        if !unlocked.contains(a) {
                            unlocked.push(*a);
                        }
                    }
                }
                let mut value = leaf::Terms::default();
                value.0.insert(
                    "expected",
                    best_plain + probability * (accepted_total - best_plain),
                );
                value.0.insert("trade_margin", -margin_group);
                value.0.insert("partner_threat", -probability * cost);
                let block = blocks(&unlocked, contested)
                    .then_some(top_threat.map(|t| t.0))
                    .flatten();
                let text = bargain::offer_text(view, &to, give, receive, &unlocked, block, false);
                rows.push(Row {
                    command: Some(settlers_protocol::Command::OfferTrade {
                        to: to.clone(),
                        give,
                        receive,
                        expires_in_ms: None,
                        message: Some(settlers_protocol::Communication {
                            intent: settlers_protocol::Intent::RequestTrade,
                            resources: Some(receive),
                            text: Some(text),
                        }),
                    }),
                    action: Action::Trade {
                        other: to[0],
                        give,
                        receive,
                    },
                    root: RootValue {
                        value,
                        trade: Some(TradeAnalysis {
                            acceptance_probability: probability,
                            acceptance_when_affordable: probability,
                            accepted_value: Some(accepted_total),
                            rejected_value: best_plain,
                            partner_affordability: 1.0,
                            unlocked_actions: unlocked,
                        }),
                    },
                    note: Some(format!(
                        "open to {}",
                        to.iter()
                            .map(|&q| bargain::name(view, q))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    speech: None,
                });
            }
        }
        // Explicit declines: an open offer to this seat that neither accepting nor countering
        // beats waiting on gets a reasoned refusal instead of silence.
        if !search.aborted
            && let Some(wait) = &wait_value
        {
            let my_open = view
                .offers
                .iter()
                .any(|offer| offer.from == seat && offer.status == OfferStatus::Open);
            let mut declined_any = false;
            for offer in &view.offers {
                if offer.status != OfferStatus::Open
                    || !offer.to.contains(&seat)
                    || offer.from == seat
                    || offer.declined.contains(&seat)
                    || my_open
                {
                    continue;
                }
                let wait_total = wait.total();
                let accept = accept_totals.get(&offer.id).copied();
                let counter = counter_best.get(&offer.id).copied();
                if accept.is_some_and(|v| v > wait_total) || counter.is_some_and(|v| v > wait_total)
                {
                    continue;
                }
                let reason = decline_reason(
                    root, seat, &knowledge, &threats, offer, accept, counter, &weights,
                );
                let mut value = wait.clone();
                value.0.insert("decline", 1e-6);
                rows.push(Row {
                    command: Some(settlers_protocol::Command::RejectTrade {
                        offer_id: offer.id.clone(),
                    }),
                    action: Action::EndTurn,
                    root: RootValue { value, trade: None },
                    note: Some(format!("decline: {reason:?}")),
                    speech: Some(bargain::decline_text(
                        view,
                        offer,
                        &reason,
                        o.turn,
                        o.settings.players,
                    )),
                });
                declined_any = true;
            }
            if !declined_any && actions.is_empty() && rows.iter().all(|r| r.command.is_some()) {
                // Waiting is the alternative to accepting an off-turn offer.
                rows.push(Row::plain(
                    None,
                    Action::EndTurn,
                    RootValue {
                        value: wait.clone(),
                        trade: None,
                    },
                ));
            }
        }
        let stats = std::mem::take(&mut search.stats);
        cumulative.transitions += stats.transitions;
        cumulative.leaves += stats.leaves;
        cumulative.afterstates += stats.afterstates;
        cumulative.expanded += stats.expanded;
        cumulative.transpositions += stats.transpositions;
        cumulative.scenarios += stats.scenarios;
        cumulative.own_nodes += stats.own_nodes;
        cumulative.cache_hits += stats.cache_hits;
        cumulative.cached_transitions += stats.cached_transitions;
        if search.aborted {
            cumulative.aborted = true;
            break;
        }
        let settled = settled_decision(&rows, config.deepen_gap);
        best_report = Some((depth, rows, stats, recorded));
        if settled || (deadline_ms > 0.0 && clock() > deadline_ms * config.deepen_fraction) {
            break;
        }
    }
    let (depth_completed, rows, stats, recorded) = match best_report {
        Some(r) => r,
        None => {
            // Even a one-turn plan was cut off: rank by static leaf so play never stalls.
            let mut fallback_cache = Cache::default();
            let mut search = Search::new(
                seat,
                weights,
                limits_of(1),
                &|| false,
                leaf.clone(),
                &mut fallback_cache,
            );
            let mut rows = Vec::new();
            for a in &actions {
                let children = search.step(&worlds, seat, *a)?;
                let value = search.leaf(&normalize(children));
                rows.push(Row::plain(
                    Some(settlers_protocol::Command::Act { action: *a }),
                    *a,
                    RootValue { value, trade: None },
                ));
            }
            (0, rows, std::mem::take(&mut search.stats), Vec::new())
        }
    };
    // The chosen plain action's expected outcomes, when explain mode recorded them.
    let mut outcomes = None;
    if config.explain {
        let best = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < recorded.len())
            .max_by(|a, b| a.1.root.value.total().total_cmp(&b.1.root.value.total()));
        let overall = rows
            .iter()
            .map(|r| r.root.value.total())
            .fold(f64::NEG_INFINITY, f64::max);
        if let Some((i, row)) = best
            && row.root.value.total() >= overall - 1e-9
        {
            outcomes = Some(summarize_outcomes(root, seat, &recorded[i], 12));
        }
    }
    let candidates: Vec<Candidate> = rows
        .into_iter()
        .map(|row| Candidate {
            id: String::new(),
            command: row.command,
            expected_value: row.root.value.total(),
            minimum_leaf_value: row.root.value.total(),
            maximum_leaf_value: row.root.value.total(),
            contributions: Contributions(row.root.value.owned()),
            trade: row.root.trade,
            principal_variation: vec![row.action],
            note: row.note,
            speech: row.speech,
        })
        .collect();
    if candidates.is_empty() && !o.legal_actions.is_empty() {
        return Err(SearchError::Horizon);
    }
    let mut stats = stats;
    stats.transitions = cumulative.transitions;
    stats.afterstates = cumulative.afterstates;
    stats.expanded = cumulative.expanded;
    stats.transpositions = cumulative.transpositions;
    stats.scenarios = cumulative.scenarios;
    stats.own_nodes = cumulative.own_nodes;
    stats.cache_hits = cumulative.cache_hits;
    stats.cached_transitions = cumulative.cached_transitions;
    stats.aborted = cumulative.aborted;
    Ok(finish_report(
        &config,
        view,
        o,
        seat,
        leaf,
        knowledge,
        candidates,
        stats,
        depth_completed,
        actions.len(),
        cache,
        clock(),
        outcomes,
    ))
}
/// Sort and number the candidates and assemble the report. `stats` carries the cumulative
/// counters in its transition, afterstate, expansion, transposition, scenario, own-node,
/// cache, and aborted fields and the last completed depth's per-level vectors.
#[allow(clippy::too_many_arguments)]
fn finish_report(
    config: &SearchConfig,
    view: &GameView,
    o: &Observation,
    seat: u8,
    leaf: &Leaf,
    knowledge: ResourceKnowledge,
    mut candidates: Vec<Candidate>,
    stats: search::Stats,
    depth_completed: u8,
    root_actions: usize,
    cache: &Cache,
    time_ms: f64,
    outcomes: Option<Vec<OutcomeLevel>>,
) -> Report {
    candidates.sort_by(|a, b| b.expected_value.total_cmp(&a.expected_value));
    for (i, c) in candidates.iter_mut().enumerate() {
        c.id = format!("decision_{i}");
    }
    let top_ids = candidates.iter().take(3).map(|c| c.id.clone()).collect();
    let players = u32::from(o.settings.players);
    Report {
        policy: if leaf.is_hand() {
            "expectimax-v2".into()
        } else {
            "expectimax-v2-tables".into()
        },
        version: view.version,
        seat,
        config: crate::Config {
            depth: config.depth,
            beam: config.beam.min(6),
            samples: config.samples.min(16),
            node_budget: config.node_budget.min(4096) as u32,
        },
        search: Some(config.clone()),
        horizon_turn: o.turn + players * u32::from(depth_completed.max(1) - 1),
        observation: o.clone(),
        offers: view.offers.clone(),
        knowledge,
        candidates,
        top_ids,
        outcomes,
        statistics: Statistics {
            decision_nodes: stats.own_nodes,
            chance_branches: stats.scenarios,
            rollout_steps: stats.transitions,
            leaves: stats.leaves,
            minimum_leaf_turn: None,
            maximum_leaf_turn: 0,
            terminal_leaves: 0,
            transitions: stats.transitions,
            afterstates: stats.afterstates,
            expanded: stats.expanded,
            transpositions: stats.transpositions,
            scenarios: stats.scenarios,
            own_nodes: stats.own_nodes,
            depth_completed: u32::from(depth_completed),
            time_ms,
            aborted: stats.aborted,
            per_level_afterstates: stats.per_level_afterstates,
            per_level_nodes: stats.per_level_nodes,
            root_actions: root_actions as u32,
            legal_actions: o.legal_actions.len() as u32,
            cache_hits: stats.cache_hits,
            cached_transitions: stats.cached_transitions,
            cache_entries: cache.entries() as u64,
        },
        assumptions: vec![
            "Depth counts the root player's planned turns; leaves are evaluated at the end of the deepest planned turn. Iterative deepening plays the deepest depth completed within the time and node budgets.".into(),
            "Opponents follow a fixed builder policy on their sampled hands under common random dice scenarios shared by every candidate. This is not an equilibrium model.".into(),
            "Hidden hands are sampled from bounded public knowledge; the root decision is shared by all sampled worlds, deeper decisions may differ between worlds.".into(),
            "Trade acceptance is the reference willingness model scaled by the trade inclination; proposals and acceptances must beat the plain alternative by the trade margin.".into(),
            "Setup placements are ranked by the opening planner: portfolios of the two sites under discrete strategies, with the intervening draft picks predicted. Values are in pips, not leaf points.".into(),
            "Contributions are additive weighted heuristic terms, not win probabilities.".into(),
        ],
    }
}
/// One partner's evaluated proposal: give, receive, partner, acceptance, accepted value,
/// threat cost, and the builds it unlocks.
type PartnerOffer = (Resources, Resources, u8, f64, f64, f64, Vec<Action>);
/// Whether one of the unlocked builds takes or touches the contested site.
fn blocks(unlocked: &[Action], contested: Option<u8>) -> bool {
    let Some(v) = contested else {
        return false;
    };
    unlocked.iter().any(|a| match a {
        Action::BuildSettlement { vertex } => *vertex == v,
        Action::BuildRoad { edge } => topology().edges[*edge as usize].contains(&v),
        _ => false,
    })
}
/// Why this seat declines `offer`, from the most to the least specific ground.
#[allow(clippy::too_many_arguments)]
fn decline_reason(
    s: &State,
    seat: u8,
    k: &ResourceKnowledge,
    threats: &[bargain::Threat; 4],
    offer: &settlers_protocol::TradeOffer,
    accept: Option<f64>,
    counter: Option<f64>,
    w: &Weights,
) -> bargain::Reason {
    let threat = threats[offer.from as usize];
    if threat.race >= 0.6 {
        return bargain::Reason::Leader {
            points: public_points(s, offer.from),
        };
    }
    if threat.total >= 0.3
        && let Some(build) = bargain::public_unlock(k, offer.from, offer.receive, offer.give)
    {
        return bargain::Reason::Unlock { build };
    }
    if threat.space >= 0.5 {
        return bargain::Reason::Space;
    }
    if let (Some(a), Some(c)) = (accept, counter)
        && c > a
    {
        return bargain::Reason::Thin;
    }
    // Keeping the asked cards for the seat's own next build.
    let own = s.players[seat as usize].resources;
    let rates = leaf::rates(s, seat);
    let _ = w;
    let asked = Resource::ALL
        .into_iter()
        .filter(|&r| offer.receive.0[r as usize] > 0)
        .max_by_key(|&r| offer.receive.0[r as usize]);
    if let Some(r) = asked {
        for (cost, build) in [
            (Resources::CITY, "a city"),
            (Resources::SETTLEMENT, "a settlement"),
            (Resources::ROAD, "a road"),
            (Resources::DEVELOPMENT, "a development card"),
        ] {
            let after = own.0[r as usize].saturating_sub(offer.receive.0[r as usize]);
            if cost.0[r as usize] > 0
                && own.0[r as usize] >= cost.0[r as usize]
                && after < cost.0[r as usize]
                && rates[r as usize] < 0.12
            {
                return bargain::Reason::Need {
                    resource: r,
                    build: Some(build),
                };
            }
        }
    }
    bargain::Reason::Plain
}
/// Per-partner multipliers on modeled willingness from this seat's own offer record:
/// `2 * (accepted + 1) / (closed + 2)`, so a partner with no history is unchanged and one
/// who accepted every offer doubles the model's willingness (capped at 1 downstream).
pub fn reciprocity(history: &[EventBatch], seat: u8) -> [f64; 4] {
    use settlers_protocol::{PublicEvent, TradeCloseReason};
    let mut offers: std::collections::BTreeMap<String, Vec<u8>> = std::collections::BTreeMap::new();
    let mut closed = [0u32; 4];
    let mut accepted = [0u32; 4];
    for batch in history {
        for event in &batch.events {
            match event {
                PublicEvent::TradeOffered { offer } if offer.from == seat => {
                    offers.insert(offer.id.clone(), offer.to.clone());
                }
                PublicEvent::TradeClosed {
                    offer_id,
                    reason,
                    winner,
                } => {
                    if let Some(to) = offers.remove(offer_id) {
                        if matches!(
                            reason,
                            TradeCloseReason::Cancelled | TradeCloseReason::Replaced
                        ) {
                            continue;
                        }
                        for r in to {
                            if (r as usize) < 4 {
                                closed[r as usize] += 1;
                            }
                        }
                        if *reason == TradeCloseReason::Accepted
                            && let Some(w) = winner
                            && (*w as usize) < 4
                        {
                            accepted[*w as usize] += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    std::array::from_fn(|q| 2.0 * f64::from(accepted[q] + 1) / f64::from(closed[q] + 2))
}
/// Rank the actor's candidate actions on a fully known state: the value of each action's
/// best line at the given depth under the leaf. Training and offline diagnostics only, never
/// seated play; the state's hidden hands reach the turn planner's chance branches but never
/// the leaf, which reads only the seat's own information.
pub fn rank_state(
    state: &State,
    seat: u8,
    config: &SearchConfig,
    leaf: &Leaf,
    depth: u8,
) -> Result<Vec<(Action, f64)>, SearchError> {
    let mut cache = Cache::default();
    rank_state_with(state, seat, config, leaf, depth, &mut cache)
}
/// `rank_state` with the caller's transposition tables, cleared according to `config.cache`.
pub fn rank_state_with(
    state: &State,
    seat: u8,
    config: &SearchConfig,
    leaf: &Leaf,
    depth: u8,
    cache: &mut Cache,
) -> Result<Vec<(Action, f64)>, SearchError> {
    let config = config.clone().bounded();
    let mut weights = config.inclinations.apply(config.weights);
    weights.endgame = config.endgame;
    let never = || false;
    match config.cache {
        CacheMode::None | CacheMode::Decision => cache.clear(),
        CacheMode::Turn => {
            if cache.owner != Some((state.turn, state.current)) {
                cache.clear();
                cache.owner = Some((state.turn, state.current));
            }
        }
    }
    let mut search = Search::new(
        seat,
        weights,
        config.limits(depth),
        &never,
        leaf.clone(),
        cache,
    );
    let mut actions = if matches!(state.phase, Phase::Discard { .. }) {
        Search::discard_candidates(state, seat, 4, config.discard_projection)
    } else {
        search.candidates(state, seat, 0)
    };
    actions.retain(|&a| apply(state, seat, a).is_ok());
    let worlds = vec![World {
        state: *state,
        weight: 1.0,
    }];
    let mut out = Vec::with_capacity(actions.len());
    for a in actions {
        let value = root_action(&mut search, &worlds, seat, a, depth)?;
        out.push((a, value.total()));
    }
    Ok(out)
}
/// Group recorded outcomes by level and by what the seat has by then (builds, cards, points,
/// game end), summing probabilities, and summarize the most likely `cap` per level relative
/// to the root position. Hands are averaged, so a researcher reads "a city at 19 and a road,
/// with probability p" rather than one entry per sampled hand.
fn summarize_outcomes(
    root: &State,
    seat: u8,
    outcomes: &[search::Outcome],
    cap: usize,
) -> Vec<OutcomeLevel> {
    type Signature = (u32, u8, Vec<u8>, Vec<u8>, Vec<u8>, u8, Option<bool>);
    type Groups = std::collections::BTreeMap<Signature, (f64, f64, f64, OutcomeSummary)>;
    let own_root = root.players[seat as usize];
    let root_dev: u8 = (0..5)
        .map(|i| own_root.development[i] + own_root.new_development[i])
        .sum();
    let describe = |s: &State| -> OutcomeSummary {
        let own = s.players[seat as usize];
        let mut settlements = Vec::new();
        let mut cities = Vec::new();
        for v in 0..54u8 {
            let before = root.board.building(v);
            let after = s.board.building(v);
            if after == seat + 1 && before != seat + 1 {
                settlements.push(v);
            }
            if after == seat + 5 && before != seat + 5 {
                cities.push(v);
            }
        }
        let roads: Vec<u8> = (0..72u8)
            .filter(|&e| s.board.road(e) == Some(seat) && root.board.road(e) != Some(seat))
            .collect();
        let dev: u8 = (0..5)
            .map(|i| own.development[i] + own.new_development[i])
            .sum();
        OutcomeSummary {
            probability: 0.0,
            value: 0.0,
            turn: s.turn,
            points: points(s, seat),
            hand: own.resources.total(),
            settlements,
            cities,
            roads,
            development: dev.saturating_sub(root_dev),
            won: match s.phase {
                Phase::Finished { winner } => Some(winner == seat),
                _ => None,
            },
        }
    };
    let mut levels: std::collections::BTreeMap<u8, Groups> = Default::default();
    for o in outcomes {
        let summary = describe(&o.state);
        let signature: Signature = (
            summary.turn,
            summary.points,
            summary.settlements.clone(),
            summary.cities.clone(),
            summary.roads.clone(),
            summary.development,
            summary.won,
        );
        let hand = f64::from(summary.hand);
        let entry = levels
            .entry(o.level)
            .or_default()
            .entry(signature)
            .or_insert((0.0, 0.0, 0.0, summary));
        entry.0 += o.probability;
        entry.1 += o.probability * o.value;
        entry.2 += o.probability * hand;
    }
    levels
        .into_iter()
        .map(|(level, groups)| {
            let mut list: Vec<(f64, f64, f64, OutcomeSummary)> = groups.into_values().collect();
            list.sort_by(|a, b| b.0.total_cmp(&a.0));
            let total: f64 = list.iter().map(|g| g.0).sum();
            let shown: Vec<OutcomeSummary> = list
                .into_iter()
                .take(cap)
                .map(|(probability, weighted, hands, mut summary)| {
                    summary.probability = probability;
                    if probability > 0.0 {
                        summary.value = weighted / probability;
                        summary.hand = (hands / probability).round() as u16;
                    }
                    summary
                })
                .collect();
            let listed: f64 = shown.iter().map(|g| g.probability).sum();
            OutcomeLevel {
                level,
                groups: shown,
                other: (total - listed).max(0.0),
            }
        })
        .collect()
}
/// Whether deepening can stop: nothing to choose between, or the best candidate leads the
/// runner-up by at least `gap` leaf units (when the rule is on).
fn settled_decision(rows: &[Row], gap: f64) -> bool {
    if rows.len() < 2 {
        return true;
    }
    if gap <= 0.0 {
        return false;
    }
    let mut values: Vec<f64> = rows.iter().map(|r| r.root.value.total()).collect();
    values.sort_by(|a, b| b.total_cmp(a));
    values[0] - values[1] >= gap
}
fn normalize(mut worlds: Vec<World>) -> Vec<World> {
    let mass: f64 = worlds.iter().map(|w| w.weight).sum();
    if mass > 0.0 {
        for w in &mut worlds {
            w.weight /= mass;
        }
    }
    worlds
}
fn root_action(
    search: &mut Search,
    worlds: &[World],
    seat: u8,
    action: Action,
    _depth: u8,
) -> Result<leaf::Terms, SearchError> {
    let children = search.step(worlds, seat, action)?;
    let mut total = leaf::Terms::default();
    let mut mass_total = 0.0;
    // Groups in first-appearance order, independent of the key function (see `regroup`).
    let mut index: std::collections::HashMap<key::Key, usize> = Default::default();
    let mut groups: Vec<Vec<World>> = Vec::new();
    for w in children {
        if w.weight > 1e-12 {
            let key = key::information_set(&w.state, seat);
            match index.get(&key) {
                Some(&i) => groups[i].push(w),
                None => {
                    index.insert(key, groups.len());
                    groups.push(vec![w]);
                }
            }
        }
    }
    let mass_all: f64 = groups.iter().flatten().map(|w| w.weight).sum();
    for group in groups {
        let mass: f64 = group.iter().map(|w| w.weight).sum();
        search.path_mass = if mass_all > 0.0 { mass / mass_all } else { 1.0 };
        let valued = search.turn_value(group, 1, 1);
        search.path_mass = 1.0;
        let (value, _) = valued?;
        total.add_scaled(&value, mass);
        mass_total += mass;
    }
    if mass_total > 0.0 {
        total.scale(1.0 / mass_total);
    }
    Ok(total)
}
