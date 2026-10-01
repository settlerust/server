#![cfg(feature = "aws")]
mod common;
use aws_sdk_dynamodb::{
    Client,
    config::{Credentials, Region},
    types::{
        AttributeDefinition, AttributeValue as A, BillingMode, KeySchemaElement, KeyType,
        ScalarAttributeType,
    },
};
use common::*;
use server::{
    service::GameService,
    storage::{Store, aws::DynamoStore, local::LocalArchive},
};
use settlers_protocol::*;
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires DynamoDB Local; set SETTLERS_TEST_DYNAMODB_URL"]
async fn ownership_manifest_restores_only_assigned_games_and_fences_old_actors() {
    let endpoint = std::env::var("SETTLERS_TEST_DYNAMODB_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:5801".into());
    let config = aws_sdk_dynamodb::config::Builder::new()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "test"))
        .endpoint_url(endpoint)
        .build();
    let db = Client::from_conf(config);
    let table = format!("ownership-test-{}", uuid::Uuid::new_v4());
    db.create_table()
        .table_name(&table)
        .billing_mode(BillingMode::PayPerRequest)
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
        .send()
        .await
        .unwrap();
    let a = Arc::new(DynamoStore::new(db.clone(), table.clone()).with_owner("a".into()));
    let b = Arc::new(DynamoStore::new(db.clone(), table.clone()).with_owner("b".into()));
    let dir = tempfile::tempdir().unwrap();
    let archive = Arc::new(LocalArchive::new(dir.path().join("archive")));
    let service = GameService::restore(a.clone(), archive.clone(), 0)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    db.put_item()
        .table_name(&table)
        .item("game_id", A::S(id.clone()))
        .item("kind", A::S("STATE".into()))
        .item("owner_id", A::S("a".into()))
        .send()
        .await
        .unwrap();
    db.put_item()
        .table_name(&table)
        .item("game_id", A::S("OWNER#a".into()))
        .item("kind", A::S(id.clone()))
        .send()
        .await
        .unwrap();
    assert_eq!(a.ids().await.unwrap(), vec![id.clone()]);
    assert!(b.ids().await.unwrap().is_empty());
    let view = service
        .create_assigned(
            &id,
            identity(0),
            CreateGame {
                name: "Owned".into(),
                settings: Settings::default(),
                host_as_player: true,
                player_name: None,
                seed: None,
                research: false,
            },
            1,
        )
        .await
        .unwrap();
    assert!(b.load(&id).await.is_err());
    // Withdraw the old owner's write permission before handing the state to B.
    db.update_item()
        .table_name(&table)
        .key("game_id", A::S(id.clone()))
        .key("kind", A::S("STATE".into()))
        .update_expression("SET owner_id = :owner")
        .expression_attribute_values(":owner", A::S("b".into()))
        .send()
        .await
        .unwrap();
    db.delete_item()
        .table_name(&table)
        .key("game_id", A::S("OWNER#a".into()))
        .key("kind", A::S(id.clone()))
        .send()
        .await
        .unwrap();
    db.put_item()
        .table_name(&table)
        .item("game_id", A::S("OWNER#b".into()))
        .item("kind", A::S(id.clone()))
        .send()
        .await
        .unwrap();
    let pending = b.pending(&id).await.unwrap().len();
    assert!(
        service
            .execute(
                &id,
                identity(1),
                request(
                    view.version,
                    Command::Join {
                        name: "Stale writer".into(),
                        spectator: false
                    }
                ),
                2
            )
            .await
            .is_err()
    );
    assert_eq!(b.pending(&id).await.unwrap().len(), pending);
    service.sync_ownership().await.unwrap();
    assert_eq!(service.loaded_games().await, 0);
    let restored = GameService::restore(b.clone(), archive, 3).await.unwrap();
    assert_eq!(restored.loaded_games().await, 1);
    assert_eq!(restored.room(&id).await.unwrap().version, view.version);
    assert!(
        restored
            .execute(
                &id,
                identity(1),
                request(
                    view.version,
                    Command::Join {
                        name: "New writer".into(),
                        spectator: false
                    }
                ),
                4
            )
            .await
            .is_ok()
    );
    db.delete_table().table_name(&table).send().await.unwrap();
}
