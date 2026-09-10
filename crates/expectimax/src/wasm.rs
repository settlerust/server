use crate::v2::{Cache, Leaf, SearchConfig};
use crate::{Config, analyze};
use serde::Deserialize;
use settlers_protocol::{EventBatch, GameView};
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

/// One independent instance per seated worker. Out-of-order/replayed event batches are safe.
#[wasm_bindgen]
#[derive(Default)]
pub struct ExpectimaxPlayer {
    history: BTreeMap<u64, EventBatch>,
    offered_turn: Option<u32>,
    cache: Cache,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    view: GameView,
    #[serde(default)]
    config: Config,
    /// When present, the tunable v2 search runs instead of the frozen reference.
    #[serde(default)]
    search: Option<SearchConfig>,
}
#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}
#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.0
}
#[wasm_bindgen]
impl ExpectimaxPlayer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn on_events(&mut self, json: &str) -> Result<(), String> {
        let batch: EventBatch = serde_json::from_str(json).map_err(|e| e.to_string())?;
        self.history.insert(batch.sequence, batch);
        Ok(())
    }
    pub fn offered(&mut self, turn: u32) {
        self.offered_turn = Some(turn);
    }
    pub fn analyze(&mut self, json: &str) -> Result<String, String> {
        let request: Request = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let history: Vec<_> = self.history.values().cloned().collect();
        let report = match &request.search {
            Some(search) => {
                let started = now_ms();
                let leaf = Leaf::from_config(search).map_err(|e| e.to_string())?;
                crate::v2::analyze_with(
                    &request.view,
                    &history,
                    search,
                    self.offered_turn,
                    &|| now_ms() - started,
                    &leaf,
                    &mut self.cache,
                )
            }
            None => analyze(&request.view, &history, request.config, self.offered_turn),
        }
        .map_err(|e| e.to_string())?;
        serde_json::to_string(&report).map_err(|e| e.to_string())
    }
}
