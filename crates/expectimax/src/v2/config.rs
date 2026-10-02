//! Search budgets and policy controls are explicit so one analysis is reproducible.
use super::{Inclinations, Weights, opening, search::Limits, weights};
use serde::{Deserialize, Serialize};
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
    /// Transposition tables: `none` rebuilds them for every depth pass,
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
/// Robber candidate scoring. With all switches off, scoring uses blocked opponent pips
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
    pub(super) fn limits(&self, depth: u8) -> Limits {
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
