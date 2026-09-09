//! Browser-compatible reference expectimax. Inputs contain only a seated observation and
//! that seat's event history. See `docs/expectimax.md` for sampling and horizon semantics.
pub mod leaf;
mod sampling;
mod search;
mod trades;
mod wasm;
pub use search::{Candidate, Config, Report, SearchError, analyze};
pub use wasm::ExpectimaxPlayer;
