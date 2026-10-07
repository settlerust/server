//! Full exports and Firehose imports are explicit offline operations.
use super::{Error, Record, Result, S3Archive, history};
use std::io::Read;

impl S3Archive {
    pub(super) async fn read_history_export(&self, id: &str) -> Result<Vec<Record>> {
        uuid::Uuid::parse_str(id).map_err(Error::storage)?;
        let mut records = Vec::new();
        for key in self.export_keys(&history::prefix(id)).await? {
            let sequence = key
                .strip_prefix(&history::prefix(id))
                .and_then(|name| name.strip_prefix("batch-"))
                .and_then(|name| name.strip_suffix(".jsonl.gz"))
                .and_then(|number| number.parse::<u64>().ok())
                .filter(|&first| first > 0 && history::first_sequence(first) == first)
                .ok_or_else(|| Error::storage("invalid history batch key"))?;
            if key != history::key(id, sequence) {
                return Err(Error::storage("invalid history batch key"));
            }
            let object = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(Error::storage)?;
            records.extend(history::decode(
                &history::read_body(object.body).await?,
                id,
                sequence,
            )?);
        }
        super::super::merge_records(records)
    }

    /// Read one selected game's Firehose export for verified import into history batches.
    pub async fn read_firehose_export(&self, id: &str) -> Result<Vec<Record>> {
        uuid::Uuid::parse_str(id).map_err(Error::storage)?;
        let mut records = Vec::new();
        for key in self.export_keys(&format!("events/game_id={id}/")).await? {
            let object = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(Error::storage)?;
            let bytes = object
                .body
                .collect()
                .await
                .map_err(Error::storage)?
                .into_bytes();
            let decoded = if bytes.starts_with(&[0x1f, 0x8b]) {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(&bytes[..])
                    .read_to_end(&mut decoded)
                    .map_err(Error::storage)?;
                decoded
            } else {
                bytes.to_vec()
            };
            for line in decoded
                .split(|&byte| byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                let record: Record = serde_json::from_slice(line).map_err(Error::storage)?;
                if record.game_id != id {
                    return Err(Error::storage("Firehose partition contains another game"));
                }
                records.push(record);
            }
        }
        super::super::merge_records(records)
    }

    async fn export_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let mut cursor = None;
        let mut keys = Vec::new();
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(cursor)
                .send()
                .await
                .map_err(Error::storage)?;
            keys.extend(
                page.contents()
                    .iter()
                    .filter_map(|object| object.key().map(str::to_owned)),
            );
            cursor = page.next_continuation_token;
            if cursor.is_none() {
                return Ok(keys);
            }
        }
    }
}
