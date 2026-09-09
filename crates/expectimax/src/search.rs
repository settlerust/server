use crate::{
    leaf::{self, Features},
    sampling, trades,
};
use serde::{Deserialize, Serialize};
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{Command, EventBatch, GameView, OfferStatus, TradeOffer};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("expectimax requires a seated, redacted observation")]
    NotSeated,
    #[error("invalid public observation")]
    InvalidObservation,
    #[error("resource or development belief is inconsistent with public counts")]
    InconsistentBelief,
    #[error("continuation could not reach the next-turn horizon")]
    Horizon,
    #[error(transparent)]
    Rule(#[from] RuleError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
/// Search depth counts the player's decisions, independently of the fair turn horizon.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub depth: u8,
    pub beam: u8,
    pub samples: u8,
    pub node_budget: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            depth: 4,
            beam: 3,
            samples: 4,
            node_budget: 96,
        }
    }
}
impl Config {
    fn bounded(self) -> Self {
        Self {
            depth: self.depth.clamp(1, 6),
            beam: self.beam.clamp(1, 6),
            samples: self.samples.clamp(1, 16),
            node_budget: self.node_budget.clamp(1, 4096),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAnalysis {
    pub acceptance_probability: f64,
    /// Conditional willingness under the uncalibrated model, separate from card availability.
    pub acceptance_when_affordable: f64,
    pub accepted_value: Option<f64>,
    pub rejected_value: f64,
    pub partner_affordability: f64,
    pub unlocked_actions: Vec<Action>,
}
/// Named additive terms whose sum is the candidate value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contributions(pub BTreeMap<String, f64>);
impl Contributions {
    pub fn total(&self) -> f64 {
        self.0.values().sum()
    }
}
impl From<Features> for Contributions {
    fn from(f: Features) -> Self {
        let value = serde_json::to_value(f).unwrap_or_default();
        Self(
            value
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_f64().map(|v| (k.clone(), v)))
                        .collect()
                })
                .unwrap_or_default(),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub command: Option<Command>,
    pub expected_value: f64,
    pub minimum_leaf_value: f64,
    pub maximum_leaf_value: f64,
    pub contributions: Contributions,
    pub trade: Option<TradeAnalysis>,
    pub principal_variation: Vec<Action>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Statistics {
    pub decision_nodes: u64,
    pub chance_branches: u64,
    pub rollout_steps: u64,
    pub leaves: u64,
    pub minimum_leaf_turn: Option<u32>,
    pub maximum_leaf_turn: u32,
    pub terminal_leaves: u64,
    // v2 counters; zero for the reference search.
    pub transitions: u64,
    pub afterstates: u64,
    pub expanded: u64,
    pub transpositions: u64,
    pub scenarios: u64,
    pub own_nodes: u64,
    pub depth_completed: u32,
    pub time_ms: f64,
    pub aborted: bool,
    pub per_level_afterstates: Vec<u64>,
    pub per_level_nodes: Vec<u64>,
    pub root_actions: u32,
    pub legal_actions: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub policy: String,
    pub version: u64,
    pub seat: u8,
    pub config: Config,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<crate::v2::SearchConfig>,
    pub horizon_turn: u32,
    pub observation: Observation,
    pub offers: Vec<TradeOffer>,
    pub knowledge: ResourceKnowledge,
    pub candidates: Vec<Candidate>,
    pub top_ids: Vec<String>,
    pub statistics: Statistics,
    pub assumptions: Vec<String>,
}
#[derive(Clone, Copy)]
struct World {
    state: State,
    weight: f64,
}
#[derive(Clone)]
struct Value {
    features: Features,
    min: f64,
    max: f64,
    line: Vec<Action>,
}
impl Value {
    fn empty() -> Self {
        Self {
            features: Features::default(),
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            line: vec![],
        }
    }
    fn add(&mut self, value: &Self, weight: f64) {
        if weight <= 0.0 {
            return;
        }
        self.features.add_scaled(value.features, weight);
        self.min = self.min.min(value.min);
        self.max = self.max.max(value.max);
        if self.line.is_empty() {
            self.line.clone_from(&value.line);
        }
    }
}
struct Search {
    seat: u8,
    horizon: u32,
    config: Config,
    stats: Statistics,
}

/// Rank actions and precomputed trade offers using only participant-visible inputs.
/// Candidate roots receive identical budgets and common deterministic sampling streams.
pub fn analyze(
    view: &GameView,
    history: &[EventBatch],
    config: Config,
    offered_turn: Option<u32>,
) -> Result<Report, SearchError> {
    if view.research_hands.is_some() {
        return Err(SearchError::NotSeated);
    }
    let o = view.game.as_ref().ok_or(SearchError::NotSeated)?;
    let seat = o.seat.ok_or(SearchError::NotSeated)?;
    let config = config.bounded();
    let mut knowledge = ResourceKnowledge::from_history_at_version(o, history, view.version);
    for offer in &view.offers {
        if offer.status == OfferStatus::Open && offer.from < o.settings.players {
            knowledge.players[offer.from as usize].condition_at_least(offer.give);
        }
    }
    knowledge.condition_on_bank(o);
    let states = sampling::worlds(o, &knowledge, config.samples)?;
    let worlds: Vec<_> = states
        .iter()
        .map(|&state| World {
            state,
            weight: 1.0 / states.len() as f64,
        })
        .collect();
    // During own turn, complete this turn and the next; off-turn, complete the upcoming one.
    let horizon = if o.turn == 0 {
        2 + u32::from((seat + o.settings.players - states[0].first) % o.settings.players)
    } else {
        o.turn
            + u32::from(if o.current == seat {
                o.settings.players
            } else {
                (seat + o.settings.players - o.current) % o.settings.players
            })
            + 1
    };
    let mut search = Search {
        seat,
        horizon,
        config,
        stats: Statistics::default(),
    };
    let mut candidates = Vec::new();
    for decision in trades::candidates(view, &knowledge, offered_turn) {
        let mut trade = None;
        let value = if decision.offer {
            let mut accepted = Vec::new();
            let mut rejected = Vec::new();
            let mut affordable = 0.0;
            let mut unlocked = Vec::new();
            for world in &worlds {
                let prob = trades::acceptance(&world.state, seat, decision.action);
                rejected.push(World {
                    state: world.state,
                    weight: world.weight * (1.0 - prob),
                });
                if let Ok(after) = apply(&world.state, seat, decision.action) {
                    affordable += world.weight;
                    for a in trades::unlocked(&world.state, &after, seat) {
                        if !unlocked.contains(&a) {
                            unlocked.push(a);
                        }
                    }
                    accepted.push(World {
                        state: after,
                        weight: world.weight * prob,
                    });
                }
            }
            let probability: f64 = accepted.iter().map(|w| w.weight).sum();
            let no = search.groups(
                &rejected,
                config.depth.saturating_sub(1),
                config.node_budget / 2,
                0,
            )?;
            let yes = if probability > 0.0 {
                Some(search.groups(
                    &accepted,
                    config.depth.saturating_sub(1),
                    config.node_budget / 2,
                    0,
                )?)
            } else {
                None
            };
            let rejected_weight: f64 = rejected.iter().map(|w| w.weight).sum();
            trade = Some(TradeAnalysis {
                acceptance_probability: probability,
                acceptance_when_affordable: if affordable > 0.0 {
                    probability / affordable
                } else {
                    0.0
                },
                accepted_value: yes.as_ref().map(|v| v.features.total() / probability),
                rejected_value: no.features.total() / rejected_weight.max(1e-12),
                partner_affordability: affordable,
                unlocked_actions: unlocked,
            });
            let mut value = Value::empty();
            value.add(&no, 1.0);
            if let Some(yes) = yes {
                value.add(&yes, 1.0);
            }
            value
        } else {
            // Publicly open offers are funded by their sender; discard impossible sampled worlds.
            let mut feasible: Vec<_> = worlds
                .iter()
                .copied()
                .filter(|w| apply(&w.state, seat, decision.action).is_ok())
                .collect();
            if feasible.is_empty() {
                continue;
            }
            let mass: f64 = feasible.iter().map(|w| w.weight).sum();
            for w in &mut feasible {
                w.weight /= mass;
            }
            let children = search.step(&feasible, seat, decision.action, config.node_budget, 0)?;
            if children.is_empty() {
                continue;
            }
            search.groups(
                &children,
                config.depth.saturating_sub(1),
                config.node_budget,
                1,
            )?
        };
        let mut line = vec![decision.action];
        line.extend(value.line);
        candidates.push(Candidate {
            id: format!("decision_{}", candidates.len()),
            command: Some(decision.command),
            expected_value: value.features.total(),
            minimum_leaf_value: value.min,
            maximum_leaf_value: value.max,
            contributions: Contributions::from(value.features),
            trade,
            principal_variation: line.into_iter().take(10).collect(),
        });
    }
    if o.legal_actions.is_empty() && !candidates.is_empty() {
        let wait = search.groups(&worlds, config.depth, config.node_budget, 0)?;
        candidates.push(Candidate {
            id: format!("decision_{}", candidates.len()),
            command: None,
            expected_value: wait.features.total(),
            minimum_leaf_value: wait.min,
            maximum_leaf_value: wait.max,
            contributions: Contributions::from(wait.features),
            trade: None,
            principal_variation: wait.line,
        });
    }
    candidates.sort_by(|a, b| {
        b.expected_value
            .total_cmp(&a.expected_value)
            .then_with(|| a.id.cmp(&b.id))
    });
    let top_ids = candidates.iter().take(3).map(|c| c.id.clone()).collect();
    Ok(Report {
        policy: "expectimax-v1".into(), version: view.version, seat, config, search: None, horizon_turn: horizon,
        observation: o.clone(), offers: view.offers.clone(),
        knowledge, candidates, top_ids, statistics: search.stats,
        assumptions: vec![
            "Depth counts own decisions; every nonterminal leaf is after the next normal turn ends.".into(),
            "Exact public bank counts constrain all sampled hands: 19 minus bank minus own hand is the other players' combined supply, not any one partner's hand. Resource estimates and sampled development cards are not exact posteriors.".into(),
            "MAX decisions are shared by worlds with the same seated observation; opponents use a deterministic own-hand/public-board builder.".into(),
            "Chance is enumerated where budget permits, otherwise sampled with reproducible common streams. Beam and rollout approximations are explicit.".into(),
            "Trade acceptance is sampled affordability times modeled willingness when affordable. Public bank abundance constrains circulating supply, not willingness; a full bank means nobody holds that resource. Acceptance remains uncalibrated.".into(),
            "Contributions sum to expected value; they are additive heuristic terms, not SHAP or causal explanations. Min/max are sampled leaf extrema, not guarantees.".into(),
        ],
    })
}

impl Search {
    fn finished(&self, s: &State) -> bool {
        matches!(s.phase, Phase::Finished { .. }) || s.turn >= self.horizon
    }
    fn leaf(&mut self, worlds: &[World]) -> Value {
        let mut value = Value::empty();
        for w in worlds {
            let f = leaf::evaluate(&w.state, self.seat);
            value.add(
                &Value {
                    features: f,
                    min: f.total(),
                    max: f.total(),
                    line: vec![],
                },
                w.weight,
            );
            self.stats.leaves += 1;
            if matches!(w.state.phase, Phase::Finished { .. }) {
                self.stats.terminal_leaves += 1;
            } else {
                self.stats.minimum_leaf_turn = Some(
                    self.stats
                        .minimum_leaf_turn
                        .map_or(w.state.turn, |n| n.min(w.state.turn)),
                );
                self.stats.maximum_leaf_turn = self.stats.maximum_leaf_turn.max(w.state.turn);
            }
        }
        value
    }
    fn groups(
        &mut self,
        worlds: &[World],
        depth: u8,
        budget: u32,
        step: u32,
    ) -> Result<Value, SearchError> {
        let mut grouped: BTreeMap<Vec<u8>, Vec<World>> = BTreeMap::new();
        for w in worlds.iter().filter(|w| w.weight > 1e-12) {
            // observe redacts RNG, future deck and opponent card types before information-set grouping.
            let key = serde_json::to_vec(&observe(&w.state, Some(self.seat)))?;
            grouped.entry(key).or_default().push(*w);
        }
        let child_budget = budget / grouped.len().max(1) as u32;
        let mut result = Value::empty();
        for (_, mut group) in grouped {
            let mass: f64 = group.iter().map(|w| w.weight).sum();
            for w in &mut group {
                w.weight /= mass;
            }
            let value = self.node(&group, depth, child_budget, step)?;
            result.add(&value, mass);
        }
        Ok(result)
    }
    fn node(
        &mut self,
        worlds: &[World],
        depth: u8,
        budget: u32,
        step: u32,
    ) -> Result<Value, SearchError> {
        let s = &worlds[0].state;
        if self.finished(s) {
            return Ok(self.leaf(worlds));
        }
        if step > 400 {
            return Err(SearchError::Horizon);
        }
        if depth == 0 || budget == 0 {
            return self.rollout(worlds, step);
        }
        let actor = actor(s);
        if actor != self.seat {
            let mut children = Vec::new();
            for world in worlds {
                let a = greedy(&world.state, actor, step)?;
                children.extend(self.step(&[*world], actor, a, budget, step)?);
            }
            return self.groups(&children, depth, budget.saturating_sub(1), step + 1);
        }
        self.stats.decision_nodes += 1;
        let mut actions = legal_actions(s, actor);
        actions.retain(|a| !matches!(a, Action::RelocateSettlement { .. }));
        actions.sort_by(|a, b| {
            leaf::action_score(s, actor, *b).total_cmp(&leaf::action_score(s, actor, *a))
        });
        actions.truncate(self.config.beam as usize);
        // Always compare spending against saving resources/end-turn.
        if s.phase == Phase::Main && !actions.contains(&Action::EndTurn) {
            actions.push(Action::EndTurn);
        }
        let child_budget = budget.saturating_sub(1) / actions.len().max(1) as u32;
        let mut best: Option<Value> = None;
        for a in actions {
            let children = self.step(worlds, actor, a, child_budget, step)?;
            let mut value = self.groups(&children, depth - 1, child_budget, step + 1)?;
            value.line.insert(0, a);
            if best
                .as_ref()
                .is_none_or(|b| value.features.total() > b.features.total())
            {
                best = Some(value);
            }
        }
        best.ok_or(SearchError::Horizon)
    }
    fn step(
        &mut self,
        worlds: &[World],
        actor: u8,
        action: Action,
        budget: u32,
        step: u32,
    ) -> Result<Vec<World>, SearchError> {
        let mut children = Vec::new();
        for (i, w) in worlds.iter().enumerate() {
            let outcomes = outcomes(&w.state, actor, action)?;
            let enumerated = outcomes.len() <= budget.max(1) as usize / worlds.len().max(1);
            if enumerated {
                for (chance, probability) in outcomes {
                    let state = transition(&w.state, actor, action, chance)?;
                    children.push(World {
                        state,
                        weight: w.weight * probability,
                    });
                }
            } else {
                let mut rng =
                    sampling::random(step.wrapping_mul(7919).wrapping_add(i as u32 * 104729 + 17));
                let chance = pick(&outcomes, &mut rng);
                children.push(World {
                    state: transition(&w.state, actor, action, chance)?,
                    weight: w.weight,
                });
            }
        }
        self.stats.chance_branches += children.len() as u64;
        Ok(children)
    }
    fn rollout(&mut self, worlds: &[World], start: u32) -> Result<Value, SearchError> {
        let mut leaves = Vec::new();
        for (i, w) in worlds.iter().enumerate() {
            let mut s = w.state;
            let mut rng = sampling::random(1709 + i as u32 * 104729 + start * 7919);
            let mut turn_steps = 0;
            let mut previous_turn = s.turn;
            for _ in 0..400 {
                if self.finished(&s) {
                    break;
                }
                if s.turn != previous_turn {
                    turn_steps = 0;
                    previous_turn = s.turn;
                }
                let p = actor(&s);
                let a = greedy(&s, p, turn_steps)?;
                let chance = pick(&outcomes(&s, p, a)?, &mut rng);
                s = transition(&s, p, a, chance)?;
                self.stats.rollout_steps += 1;
                turn_steps += 1;
            }
            if !self.finished(&s) {
                return Err(SearchError::Horizon);
            }
            leaves.push(World {
                state: s,
                weight: w.weight,
            });
        }
        Ok(self.leaf(&leaves))
    }
}
fn actor(s: &State) -> u8 {
    if let Phase::Discard { pending } = s.phase {
        pending.trailing_zeros() as u8
    } else {
        s.current
    }
}
fn greedy(s: &State, p: u8, steps: u32) -> Result<Action, SearchError> {
    if steps >= 16 && s.phase == Phase::Main {
        return Ok(Action::EndTurn);
    }
    legal_actions(s, p)
        .into_iter()
        .filter(|a| !matches!(a, Action::RelocateSettlement { .. }))
        .max_by(|a, b| leaf::action_score(s, p, *a).total_cmp(&leaf::action_score(s, p, *b)))
        .ok_or(SearchError::Horizon)
}
/// Dice totals collapse 36 equally likely ordered pairs to 11 exact weighted outcomes.
pub fn outcomes(s: &State, p: u8, a: Action) -> Result<Vec<(Option<Chance>, f64)>, SearchError> {
    if a == Action::Roll {
        return Ok((2u8..=12)
            .map(|total| {
                let first = total.saturating_sub(6).max(1);
                (
                    Some(Chance::Dice {
                        first,
                        second: total - first,
                    }),
                    f64::from(6 - total.abs_diff(7)) / 36.0,
                )
            })
            .collect());
    }
    if matches!(
        a,
        Action::BuyDevelopment
            | Action::MoveRobber {
                victim: Some(_),
                ..
            }
    ) {
        return Ok(chance_outcomes(s, p, a)?
            .into_iter()
            .map(|b| {
                (
                    Some(b.outcome),
                    f64::from(b.numerator) / f64::from(b.denominator),
                )
            })
            .collect());
    }
    Ok(vec![(None, 1.0)])
}
fn pick(outcomes: &[(Option<Chance>, f64)], rng: &mut Random) -> Option<Chance> {
    let mut n = f64::from(rng.below(1_000_000)) / 1_000_000.0;
    for &(chance, probability) in outcomes {
        if n < probability {
            return chance;
        }
        n -= probability;
    }
    outcomes.last().and_then(|x| x.0)
}
fn transition(s: &State, p: u8, a: Action, chance: Option<Chance>) -> Result<State, SearchError> {
    Ok(if let Some(c) = chance {
        apply_with_chance(s, p, a, c)?
    } else {
        apply(s, p, a)?
    })
}

#[cfg(test)]
#[path = "../tests/internal/chance.rs"]
mod chance_tests;
