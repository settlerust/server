//! Snapshot plus transactional outbox. History lives in the archive after confirmed delivery.
#[cfg(feature = "aws")]
pub mod aws;
pub mod local;
#[cfg(feature = "aws")]
pub mod research;
use crate::{
    error::Result,
    model::{Record, Room},
};
use async_trait::async_trait;
/// A commit atomically replaces current state and appends one bounded pending event.
#[async_trait]
pub trait Store: Send + Sync {
    async fn load(&self, id: &str) -> Result<Option<Room>>;
    async fn ids(&self) -> Result<Vec<String>>;
    async fn commit(&self, expected: u64, room: &Room, record: &Record) -> Result<()>;
    async fn pending(&self, id: &str) -> Result<Vec<Record>>;
    /// Called only after archive read-back verifies identical records.
    async fn prune(&self, id: &str, through: u64) -> Result<()>;
}
/// Delivery is at least once. Implementations return records deduplicated by sequence.
#[async_trait]
pub trait Archive: Send + Sync {
    async fn deliver(&self, records: &[Record]) -> Result<()>;
    async fn read(&self, id: &str) -> Result<Vec<Record>>;
}
pub const OUTBOX_LIMIT: usize = 256;
pub fn merge_records(records: impl IntoIterator<Item = Record>) -> Result<Vec<Record>> {
    let mut sorted = std::collections::BTreeMap::<u64, Record>::new();
    for record in records {
        if let Some(old) = sorted.get(&record.sequence)
            && old != &record
        {
            return Err(crate::error::Error::new(
                settlers_protocol::ErrorCode::Internal,
                "archive contains conflicting records for one sequence",
            ));
        }
        sorted.insert(record.sequence, record);
    }
    Ok(sorted.into_values().collect())
}

/// Preserve structured replay fields and expose JSON-string columns for Athena JSON functions.
pub fn encode_record(record: &Record) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(record).map_err(crate::error::Error::storage)?;
    value["input_json"] = serde_json::Value::String(
        serde_json::to_string(&record.input).map_err(crate::error::Error::storage)?,
    );
    value["public_json"] = serde_json::Value::String(
        serde_json::to_string(&record.public).map_err(crate::error::Error::storage)?,
    );
    serde_json::to_vec(&value).map_err(crate::error::Error::storage)
}

/// Identical SHA-256 routing in the control plane keeps each game's events on one source shard.
#[cfg(feature = "aws")]
pub fn firehose_batches<'a>(
    streams: &'a str,
    records: &'a [Record],
) -> std::collections::BTreeMap<&'a str, Vec<&'a Record>> {
    use sha2::{Digest, Sha256};
    let names: Vec<_> = streams.split(',').collect();
    let mut groups = std::collections::BTreeMap::<_, Vec<_>>::new();
    for record in records {
        let digest = Sha256::digest(record.game_id.as_bytes());
        let hash = digest[..8]
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
        let stream = names[(hash % names.len() as u64) as usize];
        groups.entry(stream).or_default().push(record);
    }
    groups
}
