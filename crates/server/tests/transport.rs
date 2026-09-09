mod common;
use common::*;
use server::{
    auth::Auth,
    config::now_ms,
    transport::{App, router},
};
use settlers_client::{Client, ClientFrame, ServerFrame};
use settlers_protocol::*;
async fn server(h: &Harness) -> (String, tokio::task::JoinHandle<()>) {
    let app = App {
        auth: Auth::new(vec![42; 32], true).unwrap(),
        games: h.service.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router(app)).await.unwrap();
    });
    (base, task)
}
#[tokio::test]
async fn rest_requires_jwt_and_returns_typed_errors_and_player_views() {
    let h = Harness::new().await;
    let (base, task) = server(&h).await;
    let http = reqwest::Client::new();
    let response = http.get(format!("{base}/v1/games")).send().await.unwrap();
    assert_eq!(response.status(), 401);
    let error: ApiError = response.json().await.unwrap();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    let (client, _) = Client::guest(&base, "Remote").await.unwrap();
    let view = client
        .create(&CreateGame {
            research: false,
            name: "REST game".into(),
            settings: Settings::default(),
            host_as_player: true,
            player_name: None,
            seed: None,
        })
        .await
        .unwrap();
    assert!(view.host);
    let response = http
        .post(format!("{base}/v1/games/{}/commands", view.id))
        .bearer_auth(client.token())
        .header("content-type", "application/json")
        .body("{bad")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let error: ApiError = response.json().await.unwrap();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    let players: PlayersView = http
        .get(format!("{base}/v1/games/{}/players", view.id))
        .bearer_auth(client.token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(players.members.len(), 1);
    task.abort();
}
#[tokio::test]
async fn websocket_auth_snapshot_commands_and_disconnect_use_same_identity() {
    let h = Harness::new().await;
    let (base, task) = server(&h).await;
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let token = auth.issue(&identity(0), now_ms() / 1000).unwrap();
    let client = Client::new(&base, token.token).unwrap();
    let mut socket = client.connect(&h.id).await.unwrap();
    let snapshot = socket.next().await.unwrap().unwrap();
    assert!(matches!(snapshot, ServerFrame::Snapshot { .. }));
    socket
        .send(&ClientFrame::Ping {
            nonce: "typed-ping".into(),
        })
        .await
        .unwrap();
    loop {
        if let ServerFrame::Pong { nonce } = socket.next().await.unwrap().unwrap() {
            assert_eq!(nonce, "typed-ping");
            break;
        }
    }
    assert!(
        h.room()
            .await
            .member(&identity(0).subject)
            .unwrap()
            .connected
    );
    socket.close().await.unwrap();
    for _ in 0..100 {
        if !h
            .room()
            .await
            .member(&identity(0).subject)
            .unwrap()
            .connected
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        !h.room()
            .await
            .member(&identity(0).subject)
            .unwrap()
            .connected
    );
    let mut reconnected = client.connect(&h.id).await.unwrap();
    assert!(matches!(
        reconnected.next().await.unwrap(),
        Some(ServerFrame::Snapshot { .. })
    ));
    reconnected.close().await.unwrap();
    task.abort();
}
#[tokio::test]
async fn browser_websocket_authenticates_without_putting_jwt_in_url() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let h = Harness::new().await;
    let (base, task) = server(&h).await;
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let token = auth.issue(&identity(0), now_ms() / 1000).unwrap();
    let mut request = format!("{}/v1/games/{}/ws", base.replace("http", "ws"), h.id)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("settlers-v1, bearer.{}", token.token)
            .parse()
            .unwrap(),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.headers()["sec-websocket-protocol"], "settlers-v1");
    socket.close(None).await.unwrap();
    task.abort();
}
#[tokio::test]
async fn heartbeat_detects_half_open_connection() {
    let h = Harness::new().await;
    let (base, task) = server(&h).await;
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let token = auth.issue(&identity(0), now_ms() / 1000).unwrap();
    let client = Client::new(&base, token.token).unwrap();
    let mut socket = client.connect(&h.id).await.unwrap();
    socket.next().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(25)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(
        !h.room()
            .await
            .member(&identity(0).subject)
            .unwrap()
            .connected
    );
    drop(socket);
    task.abort();
}

#[tokio::test]
async fn websocket_commands_acknowledge_commits_and_keep_every_queued_public_batch() {
    let h = Harness::new().await;
    let (base, task) = server(&h).await;
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let token = auth.issue(&identity(0), now_ms() / 1000).unwrap();
    let client = Client::new(&base, token.token).unwrap();
    let mut socket = client.connect(&h.id).await.unwrap();
    let Some(ServerFrame::Snapshot { game, .. }) = socket.next().await.unwrap() else {
        panic!()
    };
    let request = request(
        game.version,
        Command::Configure {
            settings: Settings {
                turn_seconds: 200,
                ..Settings::default()
            },
        },
    );
    let request_id = request.request_id.clone();
    socket
        .send(&ClientFrame::Command { request })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(ServerFrame::Ack { receipt }) = socket.next().await.unwrap() {
                assert_eq!(receipt.request_id, request_id);
                break;
            }
        }
    })
    .await
    .unwrap();
    let first = h
        .command(
            0,
            Command::Configure {
                settings: Settings {
                    turn_seconds: 201,
                    ..Settings::default()
                },
            },
        )
        .await
        .version;
    let second = h
        .command(
            0,
            Command::Configure {
                settings: Settings {
                    turn_seconds: 202,
                    ..Settings::default()
                },
            },
        )
        .await
        .version;
    let mut seen = vec![];
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !seen.contains(&second) {
            if let Some(ServerFrame::Events { batch }) = socket.next().await.unwrap() {
                seen.push(batch.sequence);
            }
        }
    })
    .await
    .unwrap();
    assert!(seen.contains(&first));
    socket.close().await.unwrap();
    task.abort();
}
