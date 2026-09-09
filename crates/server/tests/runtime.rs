#![cfg(feature = "aws")]
use axum::{
    Json,
    body::Bytes,
    extract::State,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn ecs_process_registers_its_task_identity_without_overwriting_placement() {
    let (send, mut receive) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let app = axum::Router::new()
        .route(
            "/metadata/stats",
            get(|| async {
                Json(json!({
                    "cpu_stats": { "cpu_usage": { "total_usage": 1000 } },
                    "memory_stats": { "usage": 10485760 }
                }))
            }),
        )
        .route(
            "/metadata/task",
            get(|| async { Json(json!({ "TaskARN": "task-test-123" })) }),
        )
        .route(
            "/",
            post(
                |State(send): State<tokio::sync::mpsc::UnboundedSender<Value>>,
                 body: Bytes| async move {
                    let input: Value = serde_json::from_slice(&body).unwrap();
                    if input.get("Item").is_some() {
                        send.send(input).unwrap();
                    }
                    Json(json!({ "Items": [] }))
                },
            ),
        )
        .with_state(send);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mock = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let _server = Process(
        Command::new(env!("CARGO_BIN_EXE_server"))
            .env_clear()
            .envs([
                ("AWS_ACCESS_KEY_ID", "test"),
                ("AWS_SECRET_ACCESS_KEY", "test"),
                ("AWS_REGION", "us-east-1"),
                ("AWS_EC2_METADATA_DISABLED", "true"),
                ("AWS_ENDPOINT_URL", endpoint.as_str()),
                ("SETTLERS_MODE", "local"),
                ("SETTLERS_STORAGE", "aws"),
                ("SETTLERS_BIND", "127.0.0.1:0"),
                ("SETTLERS_DYNAMODB_TABLE", "game-state"),
                ("SETTLERS_REGISTRATION_TABLE", "control"),
                ("SETTLERS_SERVER_ID", "server-test"),
                ("SETTLERS_FIREHOSE_STREAM", "test"),
                ("SETTLERS_ARCHIVE_BUCKET", "test"),
                ("SETTLERS_TASK_VCPU", "0.25"),
                ("SETTLERS_TASK_MEMORY_MIB", "512"),
            ])
            .env(
                "ECS_CONTAINER_METADATA_URI_V4",
                format!("{endpoint}/metadata"),
            )
            .env("SETTLERS_DATA_DIR", directory.path())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let metrics = tokio::time::timeout(Duration::from_secs(15), receive.recv())
        .await
        .unwrap()
        .unwrap();
    let registration = tokio::time::timeout(Duration::from_secs(15), receive.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metrics["TableName"], "game-state");
    assert_eq!(metrics["Item"]["kind"]["S"], "METRICS");
    assert_eq!(registration["TableName"], "control");
    assert_eq!(registration["Item"]["pk"]["S"], "REGISTRATIONS");
    assert_eq!(registration["Item"]["sk"]["S"], "server-test");
    assert_eq!(
        registration["Item"]["value"]["M"]["taskArn"]["S"],
        "task-test-123"
    );
    let timestamp = registration["Item"]["value"]["M"]["timestamp"]["N"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let expires = registration["Item"]["expiresAt"]["N"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert_eq!(expires - timestamp, 180);
    mock.abort();
}
