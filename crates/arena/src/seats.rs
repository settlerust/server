//! Seat adapters. Every adapter decides from a `GameView` plus its own event history.
use serde::{Deserialize, Serialize};
use settlers_engine::{Action, Phase, automatic_discard};
use settlers_expectimax::v2::SearchConfig;
use settlers_expectimax::{Config, analyze};
use settlers_policies::trading::TradingPolicy;
use settlers_policies::{Policy, eta_builder::EtaBuilder, fast_builder::FastBuilder};
use settlers_protocol::{Command, EventBatch, GameView};

/// Search effort counters reported by a seat after each decision.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Effort {
    pub decision_nodes: u64,
    pub chance_branches: u64,
    pub rollout_steps: u64,
    pub leaves: u64,
    pub candidates: u64,
    pub errors: u64,
    pub depth_total: u64,
    pub aborted: u64,
}
pub trait Seat: Send {
    fn label(&self) -> String;
    fn choose(&mut self, view: &GameView, history: &[EventBatch]) -> Option<Command>;
    /// Called after the table commits a command chosen by this seat.
    fn committed(&mut self, _command: &Command, _view: &GameView) {}
    fn deliver(&mut self, _batch: &EventBatch) {}
    fn effort(&mut self) -> Effort {
        Effort::default()
    }
    /// A public-safe summary of the last decision, when the seat records one.
    fn trace(&mut self) -> Option<serde_json::Value> {
        None
    }
}
/// Compact candidate summary for traces: top candidates with values and contributions.
fn summarize(report: &settlers_expectimax::Report) -> serde_json::Value {
    let o = &report.observation;
    serde_json::json!({
        "phase": o.phase,
        "hand": o.own.map(|p| p.resources),
        "position": {
            "version": report.version, "turn": o.turn, "current": o.current, "phase": o.phase,
            "hexes": o.hexes, "ports": o.ports, "buildings": o.buildings, "roads": o.roads,
            "robber": o.robber, "players": o.players, "bank": o.bank.0, "deck_count": o.deck_count,
            "longest_road": o.longest_road, "largest_army": o.largest_army,
        },
        "legal": o.legal_actions.len(),
        "depth": report.statistics.depth_completed,
        "transitions": report.statistics.transitions,
        "leaves": report.statistics.leaves,
        "afterstates": report.statistics.afterstates,
        "per_level_afterstates": report.statistics.per_level_afterstates,
        "candidates": report.candidates.iter().take(5).map(|c| serde_json::json!({
            "action": c.principal_variation.first(),
            "command": c.command.as_ref().map(|c| serde_json::to_value(c).unwrap_or_default()),
            "value": c.expected_value,
            "contributions": c.contributions,
        })).collect::<Vec<_>>(),
    })
}

/// `fast`, `eta`, `v1`, or `v1:{json config}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeatSpec {
    pub kind: String,
    pub config: Option<serde_json::Value>,
}
impl SeatSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        let (kind, config) = match text.split_once(':') {
            Some((kind, json)) => (
                kind.to_string(),
                Some(serde_json::from_str(json).map_err(|e| format!("{kind}: {e}"))?),
            ),
            None => (text.to_string(), None),
        };
        match kind.as_str() {
            "fast" | "eta" | "v1" | "v2" | "legal" => Ok(Self { kind, config }),
            _ => Err(format!("unknown seat kind {kind}")),
        }
    }
    pub fn label(&self) -> String {
        match &self.config {
            Some(config) => format!("{}:{config}", self.kind),
            None => self.kind.clone(),
        }
    }
}
pub fn build_seat(spec: &SeatSpec) -> Result<Box<dyn Seat>, String> {
    Ok(match spec.kind.as_str() {
        "fast" => Box::new(Builder::new(Box::new(FastBuilder), "fast")),
        "eta" => Box::new(Builder::new(Box::new(EtaBuilder), "eta")),
        "legal" => Box::new(LegalFirst),
        "v1" => {
            let config = match &spec.config {
                Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
                None => Config::default(),
            };
            Box::new(ReferenceExpectimax {
                config,
                offered_turn: None,
                effort: Effort::default(),
            })
        }
        "v2" => {
            let config: SearchConfig = match &spec.config {
                Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
                None => SearchConfig::default(),
            };
            Box::new(TunableExpectimax {
                config,
                offered_turn: None,
                effort: Effort::default(),
                last: None,
            })
        }
        other => return Err(format!("unknown seat kind {other}")),
    })
}

struct TunableExpectimax {
    config: SearchConfig,
    offered_turn: Option<u32>,
    effort: Effort,
    last: Option<serde_json::Value>,
}
impl Seat for TunableExpectimax {
    fn label(&self) -> String {
        format!(
            "v2:{}",
            serde_json::to_string(&self.config).unwrap_or_default()
        )
    }
    fn choose(&mut self, view: &GameView, history: &[EventBatch]) -> Option<Command> {
        let started = std::time::Instant::now();
        let clock = || started.elapsed().as_secs_f64() * 1000.0;
        match settlers_expectimax::v2::analyze(
            view,
            history,
            &self.config,
            self.offered_turn,
            &clock,
        ) {
            Ok(report) => {
                self.effort.decision_nodes += report.statistics.own_nodes;
                self.effort.chance_branches += report.statistics.scenarios;
                self.effort.rollout_steps += report.statistics.transitions;
                self.effort.leaves += report.statistics.leaves;
                self.effort.candidates += report.candidates.len() as u64;
                self.effort.depth_total += u64::from(report.statistics.depth_completed);
                self.effort.aborted += u64::from(report.statistics.aborted);
                self.last = Some(summarize(&report));
                report.candidates.into_iter().next().and_then(|c| c.command)
            }
            Err(_) => {
                self.effort.errors += 1;
                fallback(view)
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
    fn effort(&mut self) -> Effort {
        std::mem::take(&mut self.effort)
    }
    fn trace(&mut self) -> Option<serde_json::Value> {
        self.last.take()
    }
}

struct Builder {
    policy: TradingPolicy,
    label: &'static str,
}
impl Builder {
    fn new(inner: Box<dyn Policy>, label: &'static str) -> Self {
        Self {
            policy: TradingPolicy::new(inner),
            label,
        }
    }
}
impl Seat for Builder {
    fn label(&self) -> String {
        self.label.into()
    }
    fn choose(&mut self, view: &GameView, _history: &[EventBatch]) -> Option<Command> {
        self.policy.choose(view)
    }
    fn deliver(&mut self, batch: &EventBatch) {
        for event in &batch.events {
            self.policy.on_event(event);
        }
    }
}

/// Takes the first legal action; declines every offer. A legality floor, never a strategy.
struct LegalFirst;
impl Seat for LegalFirst {
    fn label(&self) -> String {
        "legal".into()
    }
    fn choose(&mut self, view: &GameView, _history: &[EventBatch]) -> Option<Command> {
        let o = view.game.as_ref()?;
        let action = o.legal_actions.first().copied()?;
        Some(Command::Act { action })
    }
}

struct ReferenceExpectimax {
    config: Config,
    offered_turn: Option<u32>,
    effort: Effort,
}
impl Seat for ReferenceExpectimax {
    fn label(&self) -> String {
        format!(
            "v1:{}",
            serde_json::to_string(&self.config).unwrap_or_default()
        )
    }
    fn choose(&mut self, view: &GameView, history: &[EventBatch]) -> Option<Command> {
        match analyze(view, history, self.config, self.offered_turn) {
            Ok(report) => {
                self.effort.decision_nodes += report.statistics.decision_nodes;
                self.effort.chance_branches += report.statistics.chance_branches;
                self.effort.rollout_steps += report.statistics.rollout_steps;
                self.effort.leaves += report.statistics.leaves;
                self.effort.candidates += report.candidates.len() as u64;
                report.candidates.into_iter().next().and_then(|c| c.command)
            }
            Err(_) => {
                self.effort.errors += 1;
                fallback(view)
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
    fn effort(&mut self) -> Effort {
        std::mem::take(&mut self.effort)
    }
}
/// Deterministic legal fallback mirroring the server's unattended-turn rule.
pub fn fallback(view: &GameView) -> Option<Command> {
    let o = view.game.as_ref()?;
    let action = match o.phase {
        Phase::Discard { .. } => Action::Discard {
            resources: automatic_discard(o.own?.resources),
        },
        Phase::Roll => Action::Roll,
        Phase::Main => Action::EndTurn,
        _ => o.legal_actions.first().copied()?,
    };
    Some(Command::Act { action })
}
