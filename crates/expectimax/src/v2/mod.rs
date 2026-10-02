//! `expectimax-v2`: a tunable turn-level expectimax with iterative deepening. See
//! `docs/expectimax.md` for the exact semantics of every setting.
pub mod bargain;
mod config;
mod evaluation;
mod opening_candidates;
mod ranking;
mod report;
mod trade_evaluation;
pub use config::*;
pub use evaluation::Leaf;
pub use ranking::{rank_state, rank_state_with};
use report::{finish_report, summarize_outcomes};
pub mod key;
pub mod leaf;
pub mod opening;
pub mod opponent;
pub mod predict;
pub mod search;
pub mod weights;
use crate::sampling;
use crate::search::{Candidate, Contributions, Report, SearchError, TradeAnalysis};
pub use search::Cache;
use search::{Search, World};
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{EventBatch, GameView, OfferStatus};
pub use weights::{Inclinations, Weights};

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
    let root = &worlds[0].state;
    let my_turn = actor(root) == seat;
    // Who is dangerous to trade with, and the site the top threat is heading for.
    let threats = bargain::threats(root, seat, config.predict.threat);
    // The one-change arms are only effective while bargaining is on; `bargain: false` turns
    // every one of them off.
    let counters = config.bargain && config.counters;
    let asks = config.bargain && config.asks;
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
        let candidates = opening_candidates::rank(root, o, &config, seat);
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
        trade_evaluation::evaluate(
            &trade_evaluation::Market {
                config: &config,
                view,
                o,
                knowledge: &knowledge,
                worlds: &worlds,
                decisions: &decisions,
                seat,
                weights,
                threats,
                partner_factor,
            },
            &mut search,
            &mut rows,
            best_plain,
            wait_value.as_ref(),
            actions.is_empty(),
        )?;
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
