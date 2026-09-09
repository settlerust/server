//! Official AWS SDK adapters. Events stay in the DynamoDB outbox until visible in S3.
use super::{Archive, Store};
use crate::{
    error::{Error, Result},
    model::{Record, Room},
};
use async_trait::async_trait;
use aws_sdk_dynamodb::{
    Client as Dynamo,
    types::{AttributeValue as A, Delete, Put, TransactWriteItem},
};
use aws_sdk_firehose::{Client as Firehose, types::Record as FirehoseRecord};
use aws_sdk_s3::Client as S3;
use std::collections::HashMap;

pub struct DynamoStore {
    client: Dynamo,
    table: String,
    assigned_game: Option<String>,
    owner: Option<String>,
}
impl DynamoStore {
    pub fn new(client: Dynamo, table: String) -> Self {
        Self {
            client,
            table,
            assigned_game: None,
            owner: None,
        }
    }
    /// Restore only this task's assigned actor, never every game in the shared table.
    pub fn with_assigned_game(mut self, id: String) -> Self {
        self.assigned_game = Some(id);
        self
    }
    /// Use a strongly consistent ownership manifest and fence every state write.
    pub fn with_owner(mut self, id: String) -> Self {
        self.owner = Some(id);
        self
    }
}
fn key(id: &str, sort: &str) -> HashMap<String, A> {
    HashMap::from([
        ("game_id".into(), A::S(id.into())),
        ("kind".into(), A::S(sort.into())),
    ])
}
fn decode<T: serde::de::DeserializeOwned>(item: &HashMap<String, A>) -> Result<T> {
    let data = item
        .get("data")
        .and_then(|a| a.as_s().ok())
        .ok_or_else(|| Error::storage("missing DynamoDB data"))?;
    serde_json::from_str(data).map_err(Error::storage)
}
#[async_trait]
impl Store for DynamoStore {
    async fn load(&self, id: &str) -> Result<Option<Room>> {
        let result = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(id, "STATE")))
            .consistent_read(true)
            .send()
            .await
            .map_err(Error::storage)?;
        let Some(item) = result.item.as_ref() else {
            return Ok(None);
        };
        if self
            .owner
            .as_ref()
            .is_some_and(|owner| item.get("owner_id").and_then(|v| v.as_s().ok()) != Some(owner))
        {
            return Err(Error::new(
                settlers_protocol::ErrorCode::Forbidden,
                "game is assigned to another server",
            ));
        }
        if !item.contains_key("data") {
            return Ok(None);
        }
        decode(item).map(Some)
    }
    async fn ids(&self) -> Result<Vec<String>> {
        if let Some(owner) = &self.owner {
            let mut cursor = None;
            let mut ids = Vec::new();
            loop {
                let page = self
                    .client
                    .query()
                    .table_name(&self.table)
                    .key_condition_expression("game_id = :owner")
                    .expression_attribute_values(":owner", A::S(format!("OWNER#{owner}")))
                    .projection_expression("#kind")
                    .expression_attribute_names("#kind", "kind")
                    .consistent_read(true)
                    .set_exclusive_start_key(cursor)
                    .send()
                    .await
                    .map_err(Error::storage)?;
                ids.extend(
                    page.items()
                        .iter()
                        .filter_map(|i| i.get("kind").and_then(|v| v.as_s().ok()).cloned()),
                );
                cursor = page.last_evaluated_key;
                if cursor.is_none() {
                    break;
                }
            }
            ids.sort();
            return Ok(ids);
        }
        if let Some(id) = &self.assigned_game {
            return Ok(vec![id.clone()]);
        }
        let mut cursor = None;
        let mut ids = vec![];
        loop {
            let result = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("#kind = :state")
                .expression_attribute_names("#kind", "kind")
                .expression_attribute_values(":state", A::S("STATE".into()))
                .projection_expression("game_id")
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(Error::storage)?;
            for item in result.items() {
                if let Some(id) = item.get("game_id").and_then(|a| a.as_s().ok()) {
                    ids.push(id.clone());
                }
            }
            cursor = result.last_evaluated_key;
            if cursor.is_none() {
                break;
            }
        }
        ids.sort();
        Ok(ids)
    }
    async fn commit(&self, expected: u64, room: &Room, record: &Record) -> Result<()> {
        if self.pending(&room.id).await?.len() >= super::OUTBOX_LIMIT {
            return Err(Error::new(
                settlers_protocol::ErrorCode::Backpressure,
                "archive horizon is full",
            ));
        }
        let state = serde_json::to_string(room).map_err(Error::storage)?;
        let event = serde_json::to_string(record).map_err(Error::storage)?;
        if state.len() > 350_000 || event.len() > 100_000 {
            return Err(Error::new(
                settlers_protocol::ErrorCode::Backpressure,
                "DynamoDB item safety limit exceeded",
            ));
        }
        let mut state_item = key(&room.id, "STATE");
        state_item.insert("data".into(), A::S(state));
        state_item.insert("version".into(), A::N(room.version.to_string()));
        if let Some(owner) = &self.owner {
            state_item.insert("owner_id".into(), A::S(owner.clone()));
        }
        let mut put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(state_item));
        if let Some(owner) = &self.owner {
            put = put
                .expression_attribute_names("#owner", "owner_id")
                .expression_attribute_values(":owner", A::S(owner.clone()));
            if expected == 0 {
                put = put
                    .condition_expression("#owner = :owner AND attribute_not_exists(#data)")
                    .expression_attribute_names("#data", "data");
            } else {
                put = put
                    .condition_expression("#owner = :owner AND #v = :expected")
                    .expression_attribute_names("#v", "version")
                    .expression_attribute_values(":expected", A::N(expected.to_string()));
            }
        } else if expected == 0 {
            put = put.condition_expression("attribute_not_exists(game_id)");
        } else {
            put = put
                .condition_expression("#v = :expected")
                .expression_attribute_names("#v", "version")
                .expression_attribute_values(":expected", A::N(expected.to_string()));
        }
        let mut item = key(&room.id, &format!("EVENT#{:020}", record.sequence));
        item.insert("data".into(), A::S(event));
        let event_put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression("attribute_not_exists(game_id)")
            .build()
            .map_err(Error::storage)?;
        let request = self
            .client
            .transact_write_items()
            .transact_items(
                TransactWriteItem::builder()
                    .put(put.build().map_err(Error::storage)?)
                    .build(),
            )
            .transact_items(TransactWriteItem::builder().put(event_put).build())
            .client_request_token(uuid::Uuid::new_v4().to_string());
        if let Err(error) = request.send().await {
            // Read back after ambiguous transport failure before declaring a commit failed.
            if let Some(actual) = self.load(&room.id).await? {
                if actual.version == room.version
                    && crate::reducer::state_hash(&actual)? == record.state_hash
                {
                    return Ok(());
                }
                if actual.version != expected {
                    return Err(Error::conflict(actual.version));
                }
            }
            return Err(Error::storage(error));
        }
        Ok(())
    }
    async fn pending(&self, id: &str) -> Result<Vec<Record>> {
        let mut cursor = None;
        let mut records = vec![];
        loop {
            let result = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("game_id = :id AND begins_with(#kind, :prefix)")
                .expression_attribute_names("#kind", "kind")
                .expression_attribute_values(":id", A::S(id.into()))
                .expression_attribute_values(":prefix", A::S("EVENT#".into()))
                .consistent_read(true)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(Error::storage)?;
            for item in result.items() {
                records.push(decode(item)?);
            }
            cursor = result.last_evaluated_key;
            if cursor.is_none() {
                break;
            }
        }
        Ok(records)
    }
    async fn prune(&self, id: &str, through: u64) -> Result<()> {
        let pending = self.pending(id).await?;
        let entries: Vec<_> = pending
            .into_iter()
            .filter(|r| r.sequence <= through)
            .collect();
        for chunk in entries.chunks(100) {
            let mut tx = self.client.transact_write_items();
            for record in chunk {
                tx = tx.transact_items(
                    TransactWriteItem::builder()
                        .delete(
                            Delete::builder()
                                .table_name(&self.table)
                                .set_key(Some(key(id, &format!("EVENT#{:020}", record.sequence))))
                                .build()
                                .map_err(Error::storage)?,
                        )
                        .build(),
                );
            }
            tx.send().await.map_err(Error::storage)?;
        }
        Ok(())
    }
}

pub struct FirehoseArchive {
    firehose: Firehose,
    s3: S3,
    stream: String,
    bucket: String,
}
impl FirehoseArchive {
    pub fn new(firehose: Firehose, s3: S3, stream: String, bucket: String) -> Self {
        Self {
            firehose,
            s3,
            stream,
            bucket,
        }
    }
}
#[async_trait]
impl Archive for FirehoseArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        // Bound batches below Firehose's 500 records / 4 MiB request limit.
        for (stream, records) in super::firehose_batches(&self.stream, records) {
            for chunk in records.chunks(25) {
                let mut request = self
                    .firehose
                    .put_record_batch()
                    .delivery_stream_name(stream);
                for record in chunk {
                    let mut bytes = super::encode_record(record)?;
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
                let response = request.send().await.map_err(Error::storage)?;
                if response.failed_put_count > 0 {
                    return Err(Error::storage(
                        "partial Firehose batch failure; retrying is safe",
                    ));
                }
            }
        }
        Ok(())
    }
    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        uuid::Uuid::parse_str(id).map_err(Error::storage)?;
        let mut cursor = None;
        let mut records = vec![];
        loop {
            let page = self
                .s3
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(format!("events/game_id={id}/"))
                .set_continuation_token(cursor)
                .send()
                .await
                .map_err(Error::storage)?;
            for object in page.contents() {
                if let Some(key) = object.key() {
                    let result = self
                        .s3
                        .get_object()
                        .bucket(&self.bucket)
                        .key(key)
                        .send()
                        .await
                        .map_err(Error::storage)?;
                    let bytes = result
                        .body
                        .collect()
                        .await
                        .map_err(Error::storage)?
                        .into_bytes();
                    let decoded = if bytes.starts_with(&[0x1f, 0x8b]) {
                        use std::io::Read;
                        let mut out = vec![];
                        flate2::read::GzDecoder::new(&bytes[..])
                            .read_to_end(&mut out)
                            .map_err(Error::storage)?;
                        out
                    } else {
                        bytes.to_vec()
                    };
                    for line in decoded
                        .split(|&b| b == b'\n')
                        .filter(|line| !line.is_empty())
                    {
                        let record: Record =
                            serde_json::from_slice(line).map_err(Error::storage)?;
                        if record.game_id != id {
                            return Err(Error::storage("archive partition contains another game"));
                        }
                        records.push(record);
                    }
                }
            }
            cursor = page.next_continuation_token;
            if cursor.is_none() {
                break;
            }
        }
        super::merge_records(records)
    }
}
