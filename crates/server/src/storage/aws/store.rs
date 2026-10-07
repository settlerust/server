use super::*;
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
        let room: Room = decode(item)?;
        room.validate()?;
        Ok(Some(room))
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
        if self.pending(&room.id).await?.len() >= super::super::OUTBOX_LIMIT {
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
