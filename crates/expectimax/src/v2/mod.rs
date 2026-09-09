//! `expectimax-v2`: a tunable turn-level expectimax with iterative deepening. See
//! `docs/expectimax.md` for the exact semantics of every setting.
pub mod key;
pub mod leaf;
pub mod opponent;
pub mod search;
pub mod weights;
use crate::search::{Candidate, Contributions, Report, SearchError, Statistics, TradeAnalysis};
use crate::{sampling, trades};
use search::{Limits, Search, World};
use serde::{Deserialize, Serialize};
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{EventBatch, GameView, OfferStatus};
pub use weights::{Inclinations, Weights};

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
        }
    }
}
struct RootValue {
    value: leaf::Terms,
    trade: Option<TradeAnalysis>,
}
type Row = (Option<settlers_protocol::Command>, Action, RootValue);
fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}

/// Rank the root player's decisions using only the seated observation and its own history.
/// `clock` returns elapsed milliseconds since the call began.
pub fn analyze(
    view: &GameView,
    history: &[EventBatch],
    config: &SearchConfig,
    offered_turn: Option<u32>,
    clock: &dyn Fn() -> f64,
) -> Result<Report, SearchError> {
    if view.research_hands.is_some() {
        return Err(SearchError::NotSeated);
    }
    let o = view.game.as_ref().ok_or(SearchError::NotSeated)?;
    let seat = o.seat.ok_or(SearchError::NotSeated)?;
    let config = config.clone().bounded();
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
    let margin = config.inclinations.trade_margin();
    let decisions = trades::candidates(view, &knowledge, offered_turn);
    let root = &worlds[0].state;
    let my_turn = actor(root) == seat;
    // Root actions come from the v2 generator; trade proposals and acceptances from the shared
    // enumerator. Discards get several candidates instead of the single automatic suggestion.
    let mut actions: Vec<Action> = Vec::new();
    if my_turn {
        let probe = Search::new(seat, weights, config.limits(1), &|| false);
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
    let mut best_report: Option<(u8, Vec<Row>, search::Stats)> = None;
    let max_depth = config.depth;
    let start_depth = if config.iterative { 1 } else { config.depth };
    let mut cumulative = search::Stats::default();
    for depth in start_depth..=max_depth {
        let mut search = Search::new(seat, weights, config.limits(depth), &over_time);
        let mut rows: Vec<Row> = Vec::new();
        let mut best_plain: Option<f64> = None;
        for a in &actions {
            let value = root_action(&mut search, &worlds, seat, *a, depth)?;
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
                    for w in &worlds {
                        let prob = trades::acceptance(&w.state, seat, d.action)
                            * (0.5 + config.inclinations.trade);
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
        if search.aborted {
            cumulative.aborted = true;
            break;
        }
        best_report = Some((depth, rows, stats));
        if deadline_ms > 0.0 && clock() > deadline_ms * 0.3 {
            break;
        }
    }
    let (depth_completed, rows, stats) = match best_report {
        Some(r) => r,
        None => {
            // Even a one-turn plan was cut off: rank by static leaf so play never stalls.
            let mut search = Search::new(seat, weights, config.limits(1), &|| false);
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
            (0, rows, std::mem::take(&mut search.stats))
        }
    };
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
        policy: "expectimax-v2".into(),
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
    let mut groups: std::collections::BTreeMap<u64, Vec<World>> = std::collections::BTreeMap::new();
    for w in children {
        if w.weight > 1e-12 {
            groups
                .entry(key::information_set(&w.state, seat))
                .or_default()
                .push(w);
        }
    }
    for (_, group) in groups {
        let mass: f64 = group.iter().map(|w| w.weight).sum();
        let (value, _) = search.turn_value(group, 1, 1)?;
        total.add_scaled(&value, mass);
        mass_total += mass;
    }
    if mass_total > 0.0 {
        total.scale(1.0 / mass_total);
    }
    Ok(total)
}
