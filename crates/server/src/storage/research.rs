//! Optional Firehose research delivery alongside the durable replay archive.
use super::Archive;
use crate::{
    error::{Error, Result},
    model::{Input, Record},
};
use async_trait::async_trait;
use aws_sdk_firehose::{Client, primitives::Blob, types::Record as FirehoseRecord};
use std::sync::Arc;

pub struct ResearchArchive {
    archive: Arc<dyn Archive>,
    client: Client,
    stream: String,
}
impl ResearchArchive {
    pub fn new(archive: Arc<dyn Archive>, client: Client, stream: String) -> Self {
        Self {
            archive,
            client,
            stream,
        }
    }
}
pub fn encode(record: &Record) -> Result<Vec<u8>> {
    let (kind, actor) = match &record.input {
        Input::Create { identity, .. } => ("created", Some(identity.subject.as_str())),
        Input::Command { identity, .. } => ("command", Some(identity.subject.as_str())),
        Input::Admission { identity, .. } => ("admission", Some(identity.subject.as_str())),
        Input::DisplayName { identity, .. } => ("display_name", Some(identity.subject.as_str())),
        Input::Presence { subject, .. } => ("presence", Some(subject.as_str())),
        Input::Activity { subject, .. } => ("activity", Some(subject.as_str())),
        Input::DisconnectTimeout => ("disconnect_timeout", None),
        Input::ComputerCommand { .. } => ("computer_command", None),
        Input::Timeout => ("timeout", None),
    };
    let value = serde_json::json!({
        "schema_version": 1, "source": "game", "event_type": kind,
        "event_id": format!("{}:{}", record.game_id, record.sequence),
        "game_id": record.game_id, "actor_id": actor,
        "occurred_at_ms": record.timestamp_ms, "sequence": record.sequence,
        "payload_json": serde_json::to_string(record).map_err(Error::storage)?,
    });
    serde_json::to_vec(&value).map_err(Error::storage)
}
#[async_trait]
impl Archive for ResearchArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        for (stream, records) in super::firehose_batches(&self.stream, records) {
            for chunk in records.chunks(25) {
                let mut request = self.client.put_record_batch().delivery_stream_name(stream);
                for record in chunk {
                    let bytes = encode(record)?;
                    if bytes.len() > 150_000 {
                        return Err(Error::storage("oversized research Firehose record"));
                    }
                    request = request.records(
                        FirehoseRecord::builder()
                            .data(Blob::new(bytes))
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
                        "partial research batch failure; outbox will retry",
                    ));
                }
            }
        }
        // Never make raw read-back eligible for pruning before research delivery succeeds.
        self.archive.deliver(records).await
    }
    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        self.archive.read_page(id, after, limit).await
    }
    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        self.archive.read(id).await
    }
}
