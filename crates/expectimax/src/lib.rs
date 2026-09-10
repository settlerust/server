//! Browser-compatible reference expectimax. Inputs contain only a seated observation and
//! that seat's event history. See `docs/expectimax.md` for sampling and horizon semantics.
pub mod leaf;
pub mod ntuple;
mod sampling;
mod search;
mod trades;
pub mod v2;
mod wasm;
pub use search::{
    Candidate, Config, Contributions, OutcomeLevel, OutcomeSummary, Report, SearchError,
    Statistics, analyze,
};
pub use wasm::ExpectimaxPlayer;
