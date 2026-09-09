//! Compact, deterministic base-game rules. See `docs/rules.md` for the rule contract.
//! `apply` returns a new state; rejection never changes its input. `apply_with_chance`
//! exposes stochastic branches to researchers without trusting client-supplied outcomes.
mod action;
mod board;
mod build;
mod cards;
mod chance;
mod economy;
mod legal;
mod random;
mod scoring;
mod state;
mod timeout;
mod topology;
mod transition;
mod view;
pub use action::*;
pub use board::*;
pub use cards::deck_order;
pub use chance::*;
pub use legal::*;
pub use random::*;
pub use scoring::*;
pub use state::*;
pub use timeout::*;
pub use topology::*;
pub use transition::*;
pub use view::*;
