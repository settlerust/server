//! Sequence-addressed S3 batches keep online history independent of game length.
use super::{Archive, Error, Record, Result, S3, history};
use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;

/// Compressed replay history with conditional, append-only batch updates.
pub struct S3Archive {
    pub(super) client: S3,
    pub(super) bucket: String,
}

struct StoredBatch {
    records: Vec<Record>,
    etag: String,
}

impl S3Archive {
    /// Store replay batches in a private S3 bucket supporting conditional writes.
    pub fn new(client: S3, bucket: String) -> Self {
        Self { client, bucket }
    }

    async fn load_batch(&self, id: &str, first: u64) -> Result<Option<StoredBatch>> {
        let result = match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(history::key(id, first))
            .send()
            .await
        {
            Ok(result) => result,
            Err(error) if error.as_service_error().is_some_and(|e| e.is_no_such_key()) => {
                return Ok(None);
            }
            Err(error) => return Err(Error::storage(error)),
        };
        let etag = result
            .e_tag
            .ok_or_else(|| Error::storage("history batch is missing its ETag"))?;
        let bytes = history::read_body(result.body).await?;
        Ok(Some(StoredBatch {
            records: history::decode(&bytes, id, first)?,
            etag,
        }))
    }

    async fn write_batch(&self, id: &str, first: u64, incoming: &[Record]) -> Result<()> {
        for _ in 0..6 {
            let current = self.load_batch(id, first).await?;
            let merged = super::super::merge_records(
                current
                    .iter()
                    .flat_map(|batch| batch.records.iter())
                    .chain(incoming)
                    .cloned(),
            )?;
            if current
                .as_ref()
                .is_some_and(|batch| batch.records == merged)
            {
                return Ok(());
            }
            let mut put = self
                .client
                .put_object()
                .bucket(&self.bucket)
                .key(history::key(id, first))
                .content_type("application/x-ndjson")
                .content_encoding("gzip")
                .body(ByteStream::from(history::encode(&merged)?));
            // An ETag fences the merge, so overlapping workers cannot erase a committed prefix.
            put = match current {
                Some(batch) => put.if_match(batch.etag),
                None => put.if_none_match("*"),
            };
            match put.send().await {
                Ok(_) => return Ok(()),
                Err(error)
                    if error.raw_response().is_some_and(|response| {
                        matches!(response.status().as_u16(), 409 | 412)
                    }) => {}
                Err(error) => return Err(Error::storage(error)),
            }
        }
        Err(Error::storage(
            "history batch changed repeatedly; outbox will retry",
        ))
    }
}

#[async_trait]
impl Archive for S3Archive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        // Validate the entire delivery before writing its first batch.
        for ((id, first), records) in history::group(records)? {
            self.write_batch(&id, first, &records).await?;
        }
        Ok(())
    }

    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        uuid::Uuid::parse_str(id).map_err(Error::storage)?;
        let Some((first, last)) = super::super::sequence_window(after, limit) else {
            return Ok(vec![]);
        };
        let mut records = Vec::new();
        let mut batch_first = history::first_sequence(first);
        loop {
            if let Some(batch) = self.load_batch(id, batch_first).await? {
                records.extend(
                    batch
                        .records
                        .into_iter()
                        .filter(|record| (first..=last).contains(&record.sequence)),
                );
            }
            let Some(next) = batch_first.checked_add(history::BATCH_RECORDS) else {
                break;
            };
            if next > last {
                break;
            }
            batch_first = next;
        }
        Ok(records)
    }

    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        self.read_history_export(id).await
    }
}
