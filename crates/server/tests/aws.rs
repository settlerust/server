#![cfg(feature = "aws")]
mod common;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use common::*;
use server::{
    reducer,
    storage::{
        Archive, Store,
        aws::{DynamoStore, FirehoseArchive},
    },
};
use settlers_protocol::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
#[tokio::test]
#[ignore = "requires DynamoDB Local at SETTLERS_TEST_DYNAMODB_URL"]
async fn dynamodb_transactions_roundtrip_reject_stale_writes_and_prune_outbox() {
    let endpoint = std::env::var("SETTLERS_TEST_DYNAMODB_URL").expect("set DynamoDB Local URL");
    let config = aws_sdk_dynamodb::config::Builder::new()
        .behavior_version_latest()
        .endpoint_url(endpoint)
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "test", "test", None, None, "tests",
        ))
        .build();
    let client = aws_sdk_dynamodb::Client::from_conf(config);
    let table = format!("settlers-test-{}", uuid::Uuid::new_v4());
    client
        .create_table()
        .table_name(&table)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("game_id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("kind")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("game_id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("kind")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap();
    let store = DynamoStore::new(client.clone(), table.clone());
    let (room, record) = reducer::create(
        uuid::Uuid::new_v4().to_string(),
        CreateGame {
            research: false,
            name: "Dynamo fixture".into(),
            settings: Settings::default(),
            host_as_player: true,
            player_name: None,
            seed: None,
        },
        identity(0),
        [0; 32],
        0,
    )
    .unwrap();
    store.commit(0, &room, &record).await.unwrap();
    assert_eq!(store.load(&room.id).await.unwrap().unwrap().version, 1);
    assert_eq!(store.pending(&room.id).await.unwrap().len(), 1);
    assert!(store.ids().await.unwrap().contains(&room.id));
    let event = server::model::Input::Command {
        identity: identity(1),
        request: request(
            1,
            Command::Join {
                name: "Bot".into(),
                spectator: false,
            },
        ),
    };
    let (next, record) = reducer::reduce(&room, event, 1).unwrap();
    store.commit(1, &next, &record).await.unwrap();
    let event = server::model::Input::Command {
        identity: identity(2),
        request: request(
            1,
            Command::Join {
                name: "Other".into(),
                spectator: false,
            },
        ),
    };
    let (stale, record) = reducer::reduce(&room, event, 1).unwrap();
    assert_eq!(
        store.commit(1, &stale, &record).await.unwrap_err().0.code,
        ErrorCode::Conflict
    );
    assert_eq!(store.pending(&room.id).await.unwrap().len(), 2);
    store.prune(&room.id, 2).await.unwrap();
    assert!(store.pending(&room.id).await.unwrap().is_empty());
    assert_eq!(store.load(&room.id).await.unwrap().unwrap().version, 2);
    client
        .delete_table()
        .table_name(table)
        .send()
        .await
        .unwrap();
}
#[derive(Clone)]
struct Mock {
    calls: Arc<AtomicUsize>,
    received: Arc<Mutex<Vec<serde_json::Value>>>,
    record: server::model::Record,
    indexed: Arc<Mutex<std::collections::BTreeMap<String, serde_json::Value>>>,
    queries: Arc<Mutex<Vec<serde_json::Value>>>,
    s3_reads: Arc<AtomicUsize>,
}
async fn aws_response(
    axum::extract::State(state): axum::extract::State<Mock>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if request.method() == axum::http::Method::POST {
        let target = request
            .headers()
            .get("x-amz-target")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let bytes = axum::body::to_bytes(request.into_body(), 1_048_576)
            .await
            .unwrap();
        let input: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if target.contains("DynamoDB") {
            let mut indexed = state.indexed.lock().unwrap();
            if target.ends_with("PutItem") {
                let key = input["Item"]["kind"]["S"].as_str().unwrap().to_owned();
                if indexed.contains_key(&key) {
                    return (axum::http::StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException","message":"exists"}))).into_response();
                }
                indexed.insert(key, input["Item"].clone());
                return axum::Json(serde_json::json!({})).into_response();
            }
            if target.ends_with("GetItem") {
                let item = indexed
                    .get(input["Key"]["kind"]["S"].as_str().unwrap())
                    .unwrap();
                return axum::Json(serde_json::json!({"Item":item})).into_response();
            }
            assert!(target.ends_with("Query"));
            state.queries.lock().unwrap().push(input.clone());
            let first = input["ExpressionAttributeValues"][":first"]["S"]
                .as_str()
                .unwrap();
            let last = input["ExpressionAttributeValues"][":last"]["S"]
                .as_str()
                .unwrap();
            let after = input["ExclusiveStartKey"]["kind"]["S"]
                .as_str()
                .unwrap_or("");
            let rows: Vec<_> = indexed
                .iter()
                .filter(|(key, _)| {
                    key.as_str() >= first && key.as_str() <= last && key.as_str() > after
                })
                .map(|(_, item)| item.clone())
                .collect();
            // Force SDK pagination even for a tiny window.
            let mut output = serde_json::json!({"Items":rows.iter().take(2).collect::<Vec<_>>()});
            if rows.len() > 2 {
                output["LastEvaluatedKey"] =
                    serde_json::json!({"game_id":rows[1]["game_id"],"kind":rows[1]["kind"]});
            }
            return axum::Json(output).into_response();
        }
        state.received.lock().unwrap().push(input);
        let n = state.calls.fetch_add(1, Ordering::SeqCst);
        return axum::Json(if n==0{serde_json::json!({"FailedPutCount":1,"RequestResponses":[{"ErrorCode":"ServiceUnavailableException","ErrorMessage":"injected"}]})}else{serde_json::json!({"FailedPutCount":0,"RequestResponses":[{"RecordId":"ok"}]})}).into_response();
    }
    state.s3_reads.fetch_add(1, Ordering::SeqCst);
    if request
        .uri()
        .query()
        .is_some_and(|q| q.contains("list-type"))
    {
        let key = format!("events/game_id={}/slice.gz", state.record.game_id);
        return ([("content-type","application/xml")],format!("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><IsTruncated>false</IsTruncated><Contents><Key>{key}</Key><Size>10</Size></Contents></ListBucketResult>")).into_response();
    }
    use std::io::Write;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    serde_json::to_writer(&mut gzip, &state.record).unwrap();
    gzip.write_all(b"\n").unwrap();
    gzip.finish().unwrap().into_response()
}
#[tokio::test]
async fn aws_sdk_firehose_handles_partial_failure_and_reads_gzip_s3_slices() {
    let (_, record) = reducer::create(
        uuid::Uuid::new_v4().to_string(),
        CreateGame {
            research: false,
            name: "AWS fixture".into(),
            settings: Settings::default(),
            host_as_player: true,
            player_name: None,
            seed: None,
        },
        identity(0),
        [0; 32],
        0,
    )
    .unwrap();
    let mock = Mock {
        calls: Arc::new(AtomicUsize::new(0)),
        received: Arc::new(Mutex::new(vec![])),
        record: record.clone(),
        indexed: Arc::new(Mutex::new(Default::default())),
        queries: Arc::new(Mutex::new(vec![])),
        s3_reads: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(aws_response)
        .with_state(mock.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let firehose = aws_sdk_firehose::Client::from_conf(
        aws_sdk_firehose::config::Builder::new()
            .behavior_version_latest()
            .endpoint_url(&endpoint)
            .region(aws_sdk_firehose::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_firehose::config::Credentials::new(
                "test", "test", None, None, "tests",
            ))
            .build(),
    );
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::new()
            .behavior_version_latest()
            .endpoint_url(&endpoint)
            .force_path_style(true)
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test", "test", None, None, "tests",
            ))
            .build(),
    );
    let dynamo = aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::config::Builder::new()
            .behavior_version_latest()
            .endpoint_url(&endpoint)
            .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                "test", "test", None, None, "tests",
            ))
            .build(),
    );
    let archive = FirehoseArchive::new(
        firehose,
        s3,
        "test-stream".into(),
        "test-bucket".into(),
        dynamo,
        "test-table".into(),
    );
    assert!(
        archive
            .deliver(std::slice::from_ref(&record))
            .await
            .is_err()
    );
    archive
        .deliver(std::slice::from_ref(&record))
        .await
        .unwrap();
    assert_eq!(
        archive.read_page(&record.game_id, 0, 1).await.unwrap(),
        vec![record.clone()]
    );
    archive
        .index_records(std::slice::from_ref(&record))
        .await
        .unwrap();
    let mut conflict = record.clone();
    conflict.public.clear();
    assert!(archive.index_records(&[conflict]).await.is_err());
    let records: Vec<_> = (2..=320)
        .map(|sequence| {
            let mut row = record.clone();
            row.sequence = sequence;
            row
        })
        .collect();
    archive.index_records(&records).await.unwrap();
    assert_eq!(
        archive
            .read_page(&record.game_id, 300, 10)
            .await
            .unwrap()
            .iter()
            .map(|r| r.sequence)
            .collect::<Vec<_>>(),
        (301..=310).collect::<Vec<_>>()
    );
    assert!(
        archive
            .read_page(&record.game_id, u64::MAX, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        archive
            .read_page(&record.game_id, 0, 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(mock.s3_reads.load(Ordering::SeqCst), 0);
    {
        let queries = mock.queries.lock().unwrap();
        assert!(queries.iter().all(|q| q["ConsistentRead"] == true));
        assert!(
            queries
                .iter()
                .skip(1)
                .all(|q| q["ExpressionAttributeValues"][":first"]["S"]
                    == "ARCHIVE#00000000000000000301"
                    && q["ExpressionAttributeValues"][":last"]["S"]
                        == "ARCHIVE#00000000000000000310")
        );
    }
    let restored = archive.read(&record.game_id).await.unwrap();
    assert_eq!(restored[0].state_hash, record.state_hash);
    use base64::Engine;
    let received = mock.received.lock().unwrap();
    let data = received[1]["Records"][0]["Data"].as_str().unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert!(decoded.ends_with(b"\n"));
    let decoded: server::model::Record = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(decoded.sequence, record.sequence);
    task.abort();
}
