//! Reporting uses completed work only; interrupted search depths cannot replace a valid result.
use super::{Cache, Leaf, SearchConfig, search};
use crate::search::{Candidate, OutcomeLevel, OutcomeSummary, Report, Statistics};
use settlers_engine::{Observation, Phase, State, points};
use settlers_protocol::{GameView, knowledge::ResourceKnowledge};
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_report(
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
/// Group recorded outcomes by level and by what the seat has by then (builds, cards, points,
/// game end), summing probabilities, and summarize the most likely `cap` per level relative
/// to the root position. Hands are averaged, so a researcher reads "a city at 19 and a road,
/// with probability p" rather than one entry per sampled hand.
pub(super) fn summarize_outcomes(
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
