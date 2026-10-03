#![cfg(feature = "aws")]
mod common;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use common::*;
use server::{
    reducer,
    storage::{Store, aws::DynamoStore},
};
use settlers_protocol::*;
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
