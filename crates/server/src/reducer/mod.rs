//! Pure room transitions: identity, trades, communication, and rule-engine adaptation.
mod admission;
mod base_v9;
mod commands;
mod creation;
mod replay;
pub use creation::create;
pub use replay::{replay, state_hash};
mod discard;
mod lobby;
mod messaging;
mod presence;
mod projection;
mod trades;
use crate::{
    auth::Identity,
    error::{Error, Result},
    model::*,
};
pub use projection::*;
use settlers_protocol::*;

/// Returns the existing receipt only when both identity and command content match.
pub fn duplicate(
    room: &Room,
    identity: &Identity,
    request: &CommandRequest,
) -> Result<Option<Receipt>> {
    if let Some(old) = room
        .receipts
        .iter()
        .find(|r| r.subject == identity.subject && r.request.request_id == request.request_id)
    {
        if old.request != *request {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "request_id was already used for a different request",
            ));
        }
        return Ok(Some(Receipt {
            request_id: request.request_id.clone(),
            version: old.version,
            duplicate: true,
        }));
    }
    Ok(None)
}
/// Validate one input with the default replay semantics and propose its durable successor.
/// Publication belongs to the actor after storage commits the snapshot and outbox together.
pub fn reduce(input: &Room, event: Input, now: u64) -> Result<(Room, Record)> {
    match ReplayVersion::CURRENT {
        ReplayVersion::BaseV9 => base_v9::reduce(input, event, now),
    }
}
