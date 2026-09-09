//! Remote seat for both expectimax searches. Decisions use only the seated view and the
//! participant event history delivered to this process.
use crate::Policy;
use settlers_engine::{Action, Observation};
use settlers_expectimax::v2::SearchConfig;
use settlers_expectimax::{Config, Report};
use settlers_protocol::{Command, EventBatch, GameView, PublicEvent};
use std::collections::BTreeMap;

enum Engine {
    Reference(Config),
    Tunable(Box<SearchConfig>),
}
pub struct ExpectimaxSeat {
    engine: Engine,
    history: BTreeMap<u64, EventBatch>,
    offered_turn: Option<u32>,
    name: &'static str,
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
            history: BTreeMap::new(),
            offered_turn: None,
        })
    }
    fn analyze(&self, view: &GameView) -> Result<Report, settlers_expectimax::SearchError> {
        let history: Vec<_> = self.history.values().cloned().collect();
        match &self.engine {
            Engine::Reference(config) => {
                settlers_expectimax::analyze(view, &history, *config, self.offered_turn)
            }
            Engine::Tunable(config) => {
                let started = std::time::Instant::now();
                settlers_expectimax::v2::analyze(view, &history, config, self.offered_turn, &|| {
                    started.elapsed().as_secs_f64() * 1000.0
                })
            }
        }
    }
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
        match self.analyze(view) {
            Ok(report) => report.candidates.into_iter().next().and_then(|c| c.command),
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
