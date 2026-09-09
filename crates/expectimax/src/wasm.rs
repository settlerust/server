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
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    view: GameView,
    #[serde(default)]
    config: Config,
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
    pub fn analyze(&self, json: &str) -> Result<String, String> {
        let request: Request = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let history: Vec<_> = self.history.values().cloned().collect();
        let report = analyze(&request.view, &history, request.config, self.offered_turn)
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&report).map_err(|e| e.to_string())
    }
}
