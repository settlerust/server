use crate::v2::{Cache, Leaf, SearchConfig};
use crate::{Config, analyze};
use serde::Deserialize;
use settlers_protocol::{EventBatch, GameView};
use std::collections::BTreeMap;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

/// One independent instance per seated worker. Out-of-order/replayed event batches are safe.
#[wasm_bindgen]
#[derive(Default)]
pub struct ExpectimaxPlayer {
    history: BTreeMap<u64, EventBatch>,
    offered_turn: Option<u32>,
    cache: Cache,
    tables: Option<(String, Arc<crate::ntuple::Model>)>,
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
/// The opening planner's placement for the seated view during setup, as an action JSON, or
/// `null` outside setup. Builders that never run the search share the same placements.
#[wasm_bindgen]
pub fn opening_placement(json: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Input {
        view: GameView,
    }
    let input: Input = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let Some(o) = &input.view.game else {
        return Ok("null".into());
    };
    let Some(seat) = o.seat else {
        return Ok("null".into());
    };
    let action = crate::v2::opening::from_observation(o)
        .and_then(|state| crate::v2::opening::place(&state, seat))
        .filter(|action| o.legal_actions.contains(action));
    serde_json::to_string(&action).map_err(|e| e.to_string())
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
    /// Install frozen tables fetched by the worker. The identifier must match `leaf.tables`.
    pub fn load_tables(&mut self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let model = crate::ntuple::Model::read_from(&mut std::io::Cursor::new(bytes), false)?;
        self.tables = Some((id.to_owned(), Arc::new(model)));
        self.cache = Cache::default();
        Ok(())
    }
    pub fn analyze(&mut self, json: &str) -> Result<String, String> {
        let request: Request = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let history: Vec<_> = self.history.values().cloned().collect();
        let report = match &request.search {
            Some(search) => {
                let started = now_ms();
                let leaf = match &search.leaf.tables {
                    Some(id) => {
                        let (_, model) = self
                            .tables
                            .as_ref()
                            .filter(|(loaded, _)| loaded == id)
                            .ok_or_else(|| format!("Learned tables {id} have not been loaded"))?;
                        Leaf::tables(Arc::clone(model), search.leaf.scale, search.leaf.hand)
                    }
                    None => Leaf::Hand,
                };
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
