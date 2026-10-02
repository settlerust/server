//! Leaf selection keeps learned evaluation optional for deterministic tests and hand tuning.
use super::{SearchConfig, Weights, leaf};
use crate::search::SearchError;
use settlers_engine::{Phase, State};
use std::sync::Arc;
#[derive(Clone)]
pub enum Leaf {
    Hand,
    Tables {
        model: Arc<crate::ntuple::Model>,
        scale: f64,
        hand: f64,
    },
}
impl Leaf {
    /// Frozen tables from memory, with the given scale and hand-written blend.
    pub fn tables(model: Arc<crate::ntuple::Model>, scale: f64, hand: f64) -> Self {
        Self::Tables { model, scale, hand }
    }
    /// Resolve a configuration: load the named tables (native builds only).
    pub fn from_config(config: &SearchConfig) -> Result<Self, SearchError> {
        let Some(path) = &config.leaf.tables else {
            return Ok(Self::Hand);
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(SearchError::Tables(
                "learned tables are not available in the browser build".into(),
            ))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let model = crate::ntuple::shared(path).map_err(SearchError::Tables)?;
            Ok(Self::Tables {
                model,
                scale: config.leaf.scale,
                hand: config.leaf.hand,
            })
        }
    }
    pub fn is_hand(&self) -> bool {
        matches!(self, Self::Hand)
    }
    /// Static value of a position from seat `p`'s perspective. A finished game is exact in
    /// both leaves; the tables never see terminal positions. `knight_play_value` shapes the
    /// hand-written development term, including the blend under a tables leaf.
    #[inline]
    pub fn evaluate(&self, s: &State, p: u8, w: &Weights, knight_play_value: bool) -> leaf::Terms {
        match self {
            Self::Hand => leaf::evaluate_with(s, p, w, knight_play_value),
            Self::Tables { model, scale, hand } => {
                let mut terms = leaf::Terms::default();
                if let Phase::Finished { winner } = s.phase {
                    terms
                        .0
                        .insert("terminal", if winner == p { *scale } else { 0.0 });
                    return terms;
                }
                terms
                    .0
                    .insert("tables", f64::from(model.value(s, p)) * scale);
                if *hand > 0.0 {
                    terms.add_scaled(&leaf::evaluate_with(s, p, w, knight_play_value), *hand);
                }
                terms
            }
        }
    }
}
