//! Remote seat for both expectimax searches. Decisions use only the seated view and the
//! participant event history delivered to this process.
use crate::Policy;
use settlers_engine::{Action, Observation, Phase};
use settlers_expectimax::v2::{Cache, SearchConfig};
use settlers_expectimax::{Config, Report, Statistics};
use settlers_protocol::{Command, EventBatch, GameView, PublicEvent};
use std::collections::BTreeMap;

/// Environment variable that turns on one JSON line per decision on stderr (see
/// [`search_trace`]). Unset, empty, or `0` leaves the seat silent.
pub const TRACE_VARIABLE: &str = "SETTLERS_SEARCH_TRACE";

enum Engine {
    Reference(Config),
    Tunable(Box<SearchConfig>),
}
pub struct ExpectimaxSeat {
    engine: Engine,
    cache: Cache,
    history: BTreeMap<u64, EventBatch>,
    offered_turn: Option<u32>,
    name: &'static str,
    trace: bool,
}
impl ExpectimaxSeat {
    /// `v1`, `v2`, `v1:{json}`, or `v2:{json}`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (kind, json) = match spec.split_once(':') {
            Some((kind, json)) => (kind, Some(json)),
            None => (spec, None),
        };
        let engine = match kind {
            "v1" => Engine::Reference(match json {
                Some(json) => serde_json::from_str(json).map_err(|e| e.to_string())?,
                None => Config::default(),
            }),
            "v2" => Engine::Tunable(Box::new(match json {
                Some(json) => serde_json::from_str(json).map_err(|e| e.to_string())?,
                None => SearchConfig::default(),
            })),
            _ => return Err(format!("unknown policy {spec}")),
        };
        Ok(Self {
            name: if matches!(engine, Engine::Reference(_)) {
                "expectimax-v1"
            } else {
                "expectimax-v2"
            },
            engine,
            cache: Cache::default(),
            history: BTreeMap::new(),
            offered_turn: None,
            trace: std::env::var_os(TRACE_VARIABLE)
                .is_some_and(|value| !value.is_empty() && value != "0"),
        })
    }
    fn analyze(&mut self, view: &GameView) -> Result<Report, settlers_expectimax::SearchError> {
        let history: Vec<_> = self.history.values().cloned().collect();
        match &self.engine {
            Engine::Reference(config) => {
                settlers_expectimax::analyze(view, &history, *config, self.offered_turn)
            }
            Engine::Tunable(config) => {
                let started = std::time::Instant::now();
                let leaf = settlers_expectimax::v2::Leaf::from_config(config)?;
                settlers_expectimax::v2::analyze_with(
                    view,
                    &history,
                    config,
                    self.offered_turn,
                    &|| started.elapsed().as_secs_f64() * 1000.0,
                    &leaf,
                    &mut self.cache,
                )
            }
        }
    }
}
/// One JSON object describing a decision for the seat's private diagnostic log: the
/// evaluator (`expectimax-v2-tables` when learned tables are the leaf), the snapshot
/// version, turn and phase, the legal and root action counts, the number of candidates, the
/// deepest completed depth, the search time and the whole decision's wall time in
/// milliseconds, whether a budget aborted a depth pass, and the own-node and transition
/// counts. It reads the report and never changes the decision.
pub fn search_trace(
    policy: &str,
    version: u64,
    turn: u32,
    phase: &Phase,
    candidates: usize,
    statistics: &Statistics,
    wall_ms: f64,
) -> serde_json::Value {
    let phase = serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.get("type").cloned())
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "trace": "search",
        "policy": policy,
        "version": version,
        "turn": turn,
        "phase": phase,
        "legal_actions": statistics.legal_actions,
        "root_actions": statistics.root_actions,
        "candidates": candidates,
        "depth_completed": statistics.depth_completed,
        "time_ms": statistics.time_ms,
        "wall_ms": wall_ms,
        "aborted": statistics.aborted,
        "own_nodes": statistics.own_nodes,
        "transitions": statistics.transitions,
    })
}
impl Policy for ExpectimaxSeat {
    fn name(&self) -> &'static str {
        self.name
    }
    fn choose_action(&mut self, observation: &Observation) -> Option<Action> {
        observation.legal_actions.first().copied()
    }
    fn on_event(&mut self, _event: &PublicEvent) {}
    fn on_batch(&mut self, batch: &EventBatch) {
        self.history.insert(batch.sequence, batch.clone());
    }
    fn choose(&mut self, view: &GameView) -> Option<Command> {
        let o = view.game.as_ref()?;
        if matches!(o.phase, settlers_engine::Phase::Finished { .. }) {
            return None;
        }
        let has_offer = view.offers.iter().any(|offer| {
            offer.status == settlers_protocol::OfferStatus::Open
                && o.seat.is_some_and(|s| offer.to.contains(&s))
        });
        if o.legal_actions.is_empty() && !has_offer {
            return None;
        }
        let started = std::time::Instant::now();
        match self.analyze(view) {
            Ok(report) => {
                if self.trace {
                    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
                    eprintln!(
                        "{}",
                        search_trace(
                            &report.policy,
                            report.version,
                            o.turn,
                            &o.phase,
                            report.candidates.len(),
                            &report.statistics,
                            wall_ms,
                        )
                    );
                }
                report.candidates.into_iter().next().and_then(|c| c.command)
            }
            Err(error) => {
                eprintln!("expectimax fallback: {error}");
                o.legal_actions
                    .first()
                    .map(|&action| Command::Act { action })
            }
        }
    }
    fn committed(&mut self, command: &Command, view: &GameView) {
        if matches!(command, Command::OfferTrade { .. })
            && let Some(o) = &view.game
        {
            self.offered_turn = Some(o.turn);
        }
    }
}
