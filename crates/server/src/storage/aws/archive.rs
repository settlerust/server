//! Firehose supplies analytics; the immutable index supplies bounded online history.
use super::*;

pub struct FirehoseArchive {
    firehose: Firehose,
    s3: S3,
    stream: String,
    bucket: String,
    index: Dynamo,
    table: String,
}
impl FirehoseArchive {
    pub fn new(
        firehose: Firehose,
        s3: S3,
        stream: String,
        bucket: String,
        index: Dynamo,
        table: String,
    ) -> Self {
        Self {
            firehose,
            s3,
            stream,
            bucket,
            index,
            table,
        }
    }
    /// Import an explicitly selected base-v9 archive without submitting it to Firehose again.
    /// Immutable keys make an interrupted import safe to resume.
    pub async fn index_records(&self, records: &[Record]) -> Result<()> {
        // Bound concurrent writes without making archive size determine live request pressure.
        for chunk in records.chunks(16) {
            let mut writes = Vec::with_capacity(chunk.len());
            for record in chunk {
                writes.push(self.index_record(record));
            }
            futures_util::future::try_join_all(writes).await?;
        }
        Ok(())
    }
    async fn index_record(&self, record: &Record) -> Result<()> {
        uuid::Uuid::parse_str(&record.game_id).map_err(Error::storage)?;
        if record.sequence == 0 || record.schema != 1 {
            return Err(Error::storage("unsupported archive record"));
        }
        let sort = format!("ARCHIVE#{:020}", record.sequence);
        let mut item = key(&record.game_id, &sort);
        item.insert(
            "data".into(),
            A::S(serde_json::to_string(record).map_err(Error::storage)?),
        );
        match self
            .index
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression("attribute_not_exists(game_id)")
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|e| e.is_conditional_check_failed_exception()) =>
            {
                let result = self
                    .index
                    .get_item()
                    .table_name(&self.table)
                    .set_key(Some(key(&record.game_id, &sort)))
                    .consistent_read(true)
                    .send()
                    .await
                    .map_err(Error::storage)?;
                let existing: Record = decode(
                    result
                        .item
                        .as_ref()
                        .ok_or_else(|| Error::storage("archive retry record missing"))?,
                )?;
                if existing != *record {
                    return Err(Error::storage("archive record conflict"));
                }
                Ok(())
            }
            Err(error) => Err(Error::storage(error)),
        }
    }
}
#[async_trait]
impl Archive for FirehoseArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        // Bound batches below Firehose's 500 records / 4 MiB request limit.
        for (stream, records) in super::super::firehose_batches(&self.stream, records) {
            for chunk in records.chunks(25) {
                let mut request = self
                    .firehose
                    .put_record_batch()
                    .delivery_stream_name(stream);
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
                let response = request.send().await.map_err(Error::storage)?;
                if response.failed_put_count > 0 {
                    return Err(Error::storage(
                        "partial Firehose batch failure; retrying is safe",
                    ));
                }
            }
        }
        self.index_records(records).await
    }
    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        uuid::Uuid::parse_str(id).map_err(Error::storage)?;
        let Some((first, last)) = super::super::sequence_window(after, limit) else {
            return Ok(vec![]);
        };
        let mut cursor = None;
        let mut records = Vec::new();
        loop {
            // The sort-key range bounds work independently of total game length or gaps.
            let page = self
                .index
                .query()
                .table_name(&self.table)
                .key_condition_expression("game_id = :game AND #kind BETWEEN :first AND :last")
                .expression_attribute_names("#kind", "kind")
                .expression_attribute_values(":game", A::S(id.into()))
                .expression_attribute_values(":first", A::S(format!("ARCHIVE#{first:020}")))
                .expression_attribute_values(":last", A::S(format!("ARCHIVE#{last:020}")))
                .consistent_read(true)
                .limit((last - first + 1) as i32)
                .set_exclusive_start_key(cursor)
                .send()
                .await
                .map_err(Error::storage)?;
            for item in page.items() {
                let record: Record = decode(item)?;
                if record.game_id != id
                    || !(first..=last).contains(&record.sequence)
                    || item.get("kind").and_then(|v| v.as_s().ok())
                        != Some(&format!("ARCHIVE#{:020}", record.sequence))
                {
                    return Err(Error::storage("archive key does not match record"));
                }
                records.push(record);
            }
            cursor = page.last_evaluated_key.filter(|key| !key.is_empty());
            if cursor.is_none() {
                break;
            }
        }
        super::super::merge_records(records)
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
        super::super::merge_records(records)
    }
}
