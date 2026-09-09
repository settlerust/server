//! Version 1 shared REST and WebSocket DTOs. No authoritative hidden state belongs here.
mod command;
mod event;
pub mod knowledge;
mod view;
pub use command::*;
pub use event::*;
pub use settlers_engine::{
    Action, Development, Event as RuleEvent, Observation, Phase, Resource, Resources, Settings,
};
pub use view::*;
pub const PROTOCOL_VERSION: u8 = 1;
