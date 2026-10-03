//! Optional analytics delivery precedes durable history delivery.
use super::{Archive, Error, Firehose, FirehoseRecord, Record, Result};
use async_trait::async_trait;
use std::sync::Arc;

/// Send raw analytics before forwarding records to the durable history archive.
pub struct FirehoseArchive {
    archive: Arc<dyn Archive>,
    client: Firehose,
    stream: String,
}

impl FirehoseArchive {
    pub fn new(archive: Arc<dyn Archive>, client: Firehose, stream: String) -> Self {
        Self {
            archive,
            client,
            stream,
        }
    }
}

#[async_trait]
impl Archive for FirehoseArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        for (stream, records) in super::super::firehose_batches(&self.stream, records) {
            for chunk in records.chunks(25) {
                let mut request = self.client.put_record_batch().delivery_stream_name(stream);
                for record in chunk {
                    let mut bytes = super::super::encode_record(record)?;
                    bytes.push(b'\n');
                    if bytes.len() > 100_000 {
                        return Err(Error::storage("oversized Firehose record"));
                    }
                    request = request.records(
                        FirehoseRecord::builder()
                            .data(aws_sdk_firehose::primitives::Blob::new(bytes))
                            .build()
                            .map_err(Error::storage)?,
                    );
                }
                if request
                    .send()
                    .await
                    .map_err(Error::storage)?
                    .failed_put_count
                    > 0
                {
                    return Err(Error::storage(
                        "partial Firehose batch failure; outbox will retry",
                    ));
                }
            }
        }
        // A failed analytics send must leave the durable outbox available to retry.
        self.archive.deliver(records).await
    }

    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        self.archive.read_page(id, after, limit).await
    }

    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        self.archive.read(id).await
    }
}
