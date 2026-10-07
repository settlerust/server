//! Fixed sequence ranges avoid manifests and bound compression and decode work.
use super::{Error, Record, Result};
use aws_sdk_s3::primitives::ByteStream;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

pub(super) const BATCH_RECORDS: u64 = 100;
const MAX_RECORD_BYTES: usize = 100_000;
const MAX_DECODED_BYTES: usize = (MAX_RECORD_BYTES + 1) * BATCH_RECORDS as usize;
const MAX_OBJECT_BYTES: usize = MAX_DECODED_BYTES + 65_536;

pub(super) fn first_sequence(sequence: u64) -> u64 {
    (sequence - 1) / BATCH_RECORDS * BATCH_RECORDS + 1
}

pub(super) fn prefix(id: &str) -> String {
    format!("history/v1/game_id={id}/")
}

pub(super) fn key(id: &str, first: u64) -> String {
    format!("{}batch-{first:020}.jsonl.gz", prefix(id))
}

pub(super) fn group(records: &[Record]) -> Result<BTreeMap<(String, u64), Vec<Record>>> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for record in records {
        validate_record(record)?;
        groups
            .entry((record.game_id.clone(), first_sequence(record.sequence)))
            .or_default()
            .push(record.clone());
    }
    for records in groups.values_mut() {
        *records = super::super::merge_records(std::mem::take(records))?;
    }
    Ok(groups)
}

fn validate_record(record: &Record) -> Result<()> {
    uuid::Uuid::parse_str(&record.game_id).map_err(Error::storage)?;
    if record.sequence == 0 || record.schema != 1 {
        return Err(Error::storage("unsupported history record"));
    }
    if serde_json::to_vec(record).map_err(Error::storage)?.len() > MAX_RECORD_BYTES {
        return Err(Error::storage("oversized history record"));
    }
    Ok(())
}

pub(super) fn encode(records: &[Record]) -> Result<Vec<u8>> {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    for record in records {
        serde_json::to_writer(&mut gzip, record).map_err(Error::storage)?;
        gzip.write_all(b"\n").map_err(Error::storage)?;
    }
    gzip.finish().map_err(Error::storage)
}

pub(super) fn decode(bytes: &[u8], id: &str, first: u64) -> Result<Vec<Record>> {
    let mut decoded = Vec::new();
    // The decoded limit also bounds highly compressed or corrupt objects.
    flate2::read::GzDecoder::new(bytes)
        .take(MAX_DECODED_BYTES as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(Error::storage)?;
    if decoded.len() > MAX_DECODED_BYTES {
        return Err(Error::storage("oversized decoded history batch"));
    }
    let mut records = Vec::new();
    let last = first.saturating_add(BATCH_RECORDS - 1);
    for line in decoded
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record: Record = serde_json::from_slice(line).map_err(Error::storage)?;
        validate_record(&record)?;
        if record.game_id != id
            || !(first..=last).contains(&record.sequence)
            || records
                .last()
                .is_some_and(|previous: &Record| previous.sequence >= record.sequence)
            || records.len() >= BATCH_RECORDS as usize
        {
            return Err(Error::storage(
                "history batch does not match its sequence key",
            ));
        }
        records.push(record);
    }
    if records.is_empty() {
        return Err(Error::storage("empty history batch"));
    }
    Ok(records)
}

pub(super) async fn read_body(mut body: ByteStream) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = body.try_next().await.map_err(Error::storage)? {
        if chunk.len() > MAX_OBJECT_BYTES.saturating_sub(bytes.len()) {
            return Err(Error::storage("oversized history object"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
