//! Replay dispatch and hashes keep persisted semantics explicit and independently testable.
use super::{base_v9, creation::create_version};
use crate::{
    error::{Error, Result},
    model::{Input, Record, ReplayVersion, Room},
};
use settlers_protocol::{ErrorCode, PublicEvent};
use sha2::{Digest, Sha256};
pub fn state_hash(room: &Room) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(room).map_err(Error::storage)?)
    ))
}
pub(super) fn record(
    room: &Room,
    input: Input,
    now: u64,
    public: Vec<PublicEvent>,
    version: ReplayVersion,
) -> Result<Record> {
    Ok(Record {
        schema: 1,
        engine_version: version,
        game_id: room.id.clone(),
        sequence: room.version,
        timestamp_ms: now,
        input,
        public,
        state_hash: state_hash(room)?,
    })
}
/// Reconstruct every persisted field and verify the archive's per-transition hash.
pub fn replay(records: &[Record]) -> Result<Room> {
    let mut room = None;
    for record in records {
        if record.schema != 1 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "unsupported replay schema",
            ));
        }
        let (next, generated) = match (&room, &record.input) {
            (
                None,
                Input::Create {
                    request,
                    identity,
                    seed,
                },
            ) => create_version(
                record.game_id.clone(),
                request.clone(),
                identity.clone(),
                *seed,
                record.timestamp_ms,
                record.engine_version,
            )?,
            (Some(previous), _) => match record.engine_version {
                ReplayVersion::BaseV9 => {
                    base_v9::reduce(previous, record.input.clone(), record.timestamp_ms)?
                }
            },
            _ => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "archive must start with lobby creation",
                ));
            }
        };
        if generated.engine_version != record.engine_version
            || next.id != record.game_id
            || next.version != record.sequence
            || state_hash(&next)? != record.state_hash
            || generated.public != record.public
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "replay sequence or state hash mismatch",
            ));
        }
        next.validate()?;
        room = Some(next);
    }
    room.ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "empty archive"))
}
