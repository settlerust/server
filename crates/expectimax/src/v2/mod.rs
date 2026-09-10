//! `expectimax-v2`: a tunable turn-level expectimax with iterative deepening. See
//! `docs/expectimax.md` for the exact semantics of every setting.
pub mod key;
pub mod leaf;
pub mod opponent;
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
    /// both leaves; the tables never see terminal positions.
    #[inline]
    pub fn evaluate(&self, s: &State, p: u8, w: &Weights) -> leaf::Terms {
        match self {
            Self::Hand => leaf::evaluate(s, p, w),
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
                    terms.add_scaled(&leaf::evaluate(s, p, w), *hand);
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
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    #[default]
    None,
    Decision,
    Turn,
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
        }
    }
}
struct RootValue {
    value: leaf::Terms,
    trade: Option<TradeAnalysis>,
}
type Row = (Option<settlers_protocol::Command>, Action, RootValue);
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
    let weights = config.inclinations.apply(config.weights);
    let partner_factor = if config.reciprocity {
        reciprocity(history, seat)
    } else {
        [1.0; 4]
    };
    let margin = config.inclinations.trade_margin();
    let mut decisions = trades::candidates(view, &knowledge, offered_turn);
    if !config.propose {
        decisions.retain(|d| !d.offer);
    }
    let root = &worlds[0].state;
    let my_turn = actor(root) == seat;
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
            config.limits(1),
            &|| false,
            leaf.clone(),
            &mut probe_cache,
        );
        actions = if matches!(root.phase, Phase::Discard { .. }) {
            Search::discard_candidates(root, seat, 4)
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
            config.limits(depth),
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
            rows.push((
                Some(settlers_protocol::Command::Act { action: *a }),
                *a,
                RootValue { value, trade: None },
            ));
        }
        if !search.aborted {
            for d in decisions
                .iter()
                .filter(|d| d.offer || matches!(d.action, Action::Trade { .. }))
            {
                if search.aborted {
                    break;
                }
                let Action::Trade { .. } = d.action else {
                    continue;
                };
                let value = if d.offer {
                    let Some(best_plain) = best_plain else {
                        continue;
                    };
                    let mut accepted = Vec::new();
                    let mut affordable = 0.0;
                    let mut unlocked = Vec::new();
                    let partner = match d.action {
                        Action::Trade { other, .. } => other as usize,
                        _ => 0,
                    };
                    for w in &worlds {
                        let prob = trades::acceptance(&w.state, seat, d.action)
                            * (0.5 + config.inclinations.trade)
                            * partner_factor[partner];
                        if let Ok(after) = apply(&w.state, seat, d.action) {
                            affordable += w.weight;
                            for a in trades::unlocked(&w.state, &after, seat) {
                                if !unlocked.contains(&a) {
                                    unlocked.push(a);
                                }
                            }
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
                    let accepted_value = search.plan(accepted, 1)?.0;
                    if search.aborted {
                        break;
                    }
                    let accepted_total = accepted_value.total();
                    let mut value = leaf::Terms::default();
                    value.0.insert(
                        "expected",
                        best_plain + probability * (accepted_total - best_plain),
                    );
                    value.0.insert("trade_margin", -margin);
                    RootValue {
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
                    }
                } else {
                    // Accepting an open offer: only worlds where the partner can pay.
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
                    let mut value = if actor(&feasible[0].state) == seat
                        && feasible[0].state.phase == Phase::Main
                    {
                        search.plan(feasible, 1)?.0
                    } else {
                        search.round(&normalize(feasible), 0)?
                    };
                    value.0.insert("trade_margin", -margin);
                    RootValue { value, trade: None }
                };
                rows.push((Some(d.command.clone()), d.action, value));
            }
        }
        if !search.aborted && actions.is_empty() && rows.iter().all(|r| r.0.is_some()) && !my_turn {
            // Waiting is the alternative to accepting an off-turn offer.
            let value = search.round(&worlds, 0)?;
            rows.push((None, Action::EndTurn, RootValue { value, trade: None }));
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
                config.limits(1),
                &|| false,
                leaf.clone(),
                &mut fallback_cache,
            );
            let mut rows = Vec::new();
            for a in &actions {
                let children = search.step(&worlds, seat, *a)?;
                let value = search.leaf(&normalize(children));
                rows.push((
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
            .max_by(|a, b| a.1.2.value.total().total_cmp(&b.1.2.value.total()));
        let overall = rows
            .iter()
            .map(|r| r.2.value.total())
            .fold(f64::NEG_INFINITY, f64::max);
        if let Some((i, row)) = best
            && row.2.value.total() >= overall - 1e-9
        {
            outcomes = Some(summarize_outcomes(root, seat, &recorded[i], 12));
        }
    }
    let mut candidates: Vec<Candidate> = rows
        .into_iter()
        .map(|(command, action, root)| Candidate {
            id: String::new(),
            command,
            expected_value: root.value.total(),
            minimum_leaf_value: root.value.total(),
            maximum_leaf_value: root.value.total(),
            contributions: Contributions(root.value.owned()),
            trade: root.trade,
            principal_variation: vec![action],
        })
        .collect();
    candidates.sort_by(|a, b| b.expected_value.total_cmp(&a.expected_value));
    for (i, c) in candidates.iter_mut().enumerate() {
        c.id = format!("decision_{i}");
    }
    if candidates.is_empty() && !o.legal_actions.is_empty() {
        return Err(SearchError::Horizon);
    }
    let top_ids = candidates.iter().take(3).map(|c| c.id.clone()).collect();
    let players = u32::from(o.settings.players);
    Ok(Report {
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
        search: Some(config),
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
            transitions: cumulative.transitions,
            afterstates: cumulative.afterstates,
            expanded: cumulative.expanded,
            transpositions: cumulative.transpositions,
            scenarios: cumulative.scenarios,
            own_nodes: cumulative.own_nodes,
            depth_completed: u32::from(depth_completed),
            time_ms: clock(),
            aborted: cumulative.aborted,
            per_level_afterstates: stats.per_level_afterstates,
            per_level_nodes: stats.per_level_nodes,
            root_actions: actions.len() as u32,
            legal_actions: o.legal_actions.len() as u32,
            cache_hits: cumulative.cache_hits,
            cached_transitions: cumulative.cached_transitions,
            cache_entries: cache.entries() as u64,
        },
        assumptions: vec![
            "Depth counts the root player's planned turns; leaves are evaluated at the end of the deepest planned turn. Iterative deepening plays the deepest depth completed within the time and node budgets.".into(),
            "Opponents follow a fixed builder policy on their sampled hands under common random dice scenarios shared by every candidate. This is not an equilibrium model.".into(),
            "Hidden hands are sampled from bounded public knowledge; the root decision is shared by all sampled worlds, deeper decisions may differ between worlds.".into(),
            "Trade acceptance is the reference willingness model scaled by the trade inclination; proposals and acceptances must beat the plain alternative by the trade margin.".into(),
            "Contributions are additive weighted heuristic terms, not win probabilities.".into(),
        ],
    })
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
    let weights = config.inclinations.apply(config.weights);
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
        Search::discard_candidates(state, seat, 4)
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
    let mut values: Vec<f64> = rows.iter().map(|r| r.2.value.total()).collect();
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
