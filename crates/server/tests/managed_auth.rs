mod common;
use common::*;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use server::{
    auth::Auth,
    config::now_ms,
    managed_auth::{Grant, Role},
    transport::{App, router},
};
use settlers_protocol::*;
const PRIVATE: &[u8] = include_bytes!("fixtures/jwt-test-private.pem");
const PUBLIC: &[u8] = include_bytes!("fixtures/jwt-test-public.pem");
const ID: &str = "d7dbd5f7-8b75-4c8a-879f-eb2d2896fbc6";
fn grant(id: &str, role: Role, now: u64) -> Grant {
    Grant {
        iss: "https://example.test".into(),
        aud: "settlers-v1".into(),
        sub: "player-0".into(),
        name: "Player 0".into(),
        name_revision: None,
        iat: now,
        nbf: now,
        exp: now + 900,
        game_id: id.into(),
        server_id: None,
        role,
    }
}
fn token(grant: &Grant) -> String {
    encode(
        &Header::new(Algorithm::RS256),
        grant,
        &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
    )
    .unwrap()
}
async fn http(
    app: &axum::Router,
    method: &str,
    path: &str,
    jwt: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if let Some(jwt) = jwt {
        request = request.header("Authorization", format!("Bearer {jwt}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(
                    body.map_or_else(String::new, |v| v.to_string()),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[test]
fn external_tokens_cannot_be_minted_and_are_bound_to_issuer_game_and_expiry() {
    let auth = Auth::verification_only(PUBLIC, "https://example.test", ID).unwrap();
    let valid = grant(ID, Role::Player, 100);
    assert_eq!(auth.verify(&token(&valid), 101).unwrap(), identity(0));
    assert!(auth.issue(&identity(0), 100).is_err());
    for bad in [
        Grant {
            iss: "wrong".into(),
            ..valid.clone()
        },
        Grant {
            aud: "wrong".into(),
            ..valid.clone()
        },
        Grant {
            game_id: uuid::Uuid::new_v4().to_string(),
            ..valid.clone()
        },
        Grant {
            exp: 101,
            ..valid.clone()
        },
        Grant {
            nbf: 102,
            ..valid.clone()
        },
        Grant {
            iat: 102,
            ..valid.clone()
        },
        Grant {
            exp: valid.iat + 30 * 86400 + 1,
            ..valid.clone()
        },
    ] {
        assert!(auth.verify(&token(&bad), 101).is_err());
    }
    let local = Auth::new(vec![42; 32], true)
        .unwrap()
        .issue(&identity(0), 100)
        .unwrap();
    assert!(auth.verify(&local.token, 101).is_err());
    assert!(auth.verify(&(token(&valid) + "a"), 101).is_err());
}
#[test]
fn external_tokens_remain_valid_for_thirty_days_on_assigned_and_shared_servers() {
    let server_id = uuid::Uuid::new_v4().to_string();
    let mut valid = grant(ID, Role::Player, 100);
    valid.exp = valid.iat + 30 * 86400;
    valid.server_id = Some(server_id.clone());
    for auth in [
        Auth::verification_only(PUBLIC, "https://example.test", ID).unwrap(),
        Auth::verification_for_server(PUBLIC, "https://example.test", &server_id).unwrap(),
    ] {
        let signed = token(&valid);
        for time in [valid.iat + 3601, valid.iat + 86400, valid.exp - 1] {
            assert_eq!(auth.verify(&signed, time).unwrap(), identity(0));
        }
        assert!(auth.verify(&signed, valid.exp).is_err());
        let too_long = Grant {
            exp: valid.exp + 1,
            ..valid.clone()
        };
        assert!(auth.verify(&token(&too_long), valid.iat).is_err());
    }
}
#[tokio::test]
async fn shared_server_hosts_distinct_games_but_rejects_cross_game_and_cross_server_grants() {
    let server_id = uuid::Uuid::new_v4().to_string();
    let other_id = uuid::Uuid::new_v4().to_string();
    let auth = Auth::verification_for_server(PUBLIC, "https://example.test", &server_id).unwrap();
    let h = Harness::new().await;
    let router = router(App {
        auth: auth.clone(),
        games: h.service.clone(),
    });
    for id in [ID, &other_id] {
        let mut scope = grant(id, Role::Host, now_ms() / 1000);
        scope.server_id = Some(server_id.clone());
        let token = token(&scope);
        assert!(auth.verify(&token, now_ms() / 1000).is_ok());
        let response = http(&router, "POST", "/v1/games", Some(&token), Some(serde_json::json!({"name":"Shared", "settings":Settings::default(), "host_as_player":true}))).await;
        assert_eq!(response.0, 200);
        assert_eq!(response.1["id"], id);
        let wrong_path = if id == ID { &other_id } else { ID };
        assert_eq!(
            http(
                &router,
                "GET",
                &format!("/v1/games/{wrong_path}"),
                Some(&token),
                None
            )
            .await
            .0,
            403
        );
        scope.server_id = Some(uuid::Uuid::new_v4().to_string());
        assert!(auth.verify(&crate::token(&scope), now_ms() / 1000).is_err());
        scope.server_id = None;
        assert!(auth.verify(&crate::token(&scope), now_ms() / 1000).is_err());
    }
}
#[tokio::test]
async fn assigned_creation_recovers_concurrent_retries_without_duplicate_games() {
    let h = Harness::new().await;
    let request = CreateGame {
        research: false,
        name: "Assigned".into(),
        settings: Settings::default(),
        host_as_player: true,
        player_name: None,
        seed: None,
    };
    let (a, b) = tokio::join!(
        h.service
            .create_assigned(ID, identity(0), request.clone(), 1),
        h.service
            .create_assigned(ID, identity(0), request.clone(), 1)
    );
    assert_eq!(a.unwrap().id, b.unwrap().id);
    assert_eq!(h.service.room(ID).await.unwrap().version, 1);
    assert!(
        h.service
            .create_assigned(ID, identity(1), request, 1)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn rest_and_websocket_enforce_spectator_grants_and_disable_issuance() {
    let h = Harness::new().await;
    let auth = Auth::verification_only(PUBLIC, "https://example.test", &h.id).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = App {
        auth,
        games: h.service.clone(),
    };
    let task = tokio::spawn(async move {
        axum::serve(listener, router(app)).await.unwrap();
    });
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.post(format!("{base}/v1/auth/guest"))
            .json(&serde_json::json!({"name":"Guest"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut spectator = grant(&h.id, Role::Spectator, now_ms() / 1000);
    spectator.sub = "observer".into();
    spectator.name = "Observer".into();
    let jwt = token(&spectator);
    let command = request(
        1,
        Command::Join {
            name: "Observer".into(),
            spectator: false,
        },
    );
    assert_eq!(
        http.post(format!("{base}/v1/games/{}/commands", h.id))
            .bearer_auth(&jwt)
            .json(&command)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.post(format!("{base}/v1/auth/refresh"))
            .bearer_auth(&jwt)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.post(format!("{base}/v1/games"))
            .bearer_auth(&jwt)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let client = settlers_client::Client::new(&base, jwt).unwrap();
    client
        .submit(
            &h.id,
            &request(
                1,
                Command::Join {
                    name: "Observer".into(),
                    spectator: true,
                },
            ),
        )
        .await
        .unwrap();
    let mut socket = client.connect(&h.id).await.unwrap();
    socket.next().await.unwrap().unwrap();
    socket
        .send(&ClientFrame::Command {
            request: request(
                2,
                Command::Join {
                    name: "Observer".into(),
                    spectator: false,
                },
            ),
        })
        .await
        .unwrap();
    loop {
        if let ServerFrame::Error { error, .. } = socket.next().await.unwrap().unwrap() {
            assert_eq!(error.code, ErrorCode::Forbidden);
            break;
        }
    }
    socket.close().await.unwrap();
    task.abort();
}

#[tokio::test]
async fn managed_names_are_signed_versioned_and_cannot_be_overridden_by_request_bodies() {
    let h = Harness::new().await;
    let app = router(App {
        auth: Auth::verification_only(PUBLIC, "https://example.test", ID).unwrap(),
        games: h.service.clone(),
    });
    let mut host = grant(ID, Role::Host, now_ms() / 1000);
    host.sub = "google:host".into();
    host.name = "Maya C".into();
    host.name_revision = Some(1);
    let jwt = token(&host);
    let created = http(&app, "POST", "/v1/games", Some(&jwt), Some(serde_json::json!({
        "name": "Privacy", "settings": Settings::default(), "host_as_player": true, "player_name": "Maya Louise Chen"
    }))).await;
    assert_eq!(created.0, 200);
    assert_eq!(created.1["members"][0]["name"], "Maya C");
    assert!(!created.1.to_string().contains("Louise"));
    host.name = "MC".into();
    host.name_revision = Some(2);
    let changed = http(
        &app,
        "GET",
        &format!("/v1/games/{ID}"),
        Some(&token(&host)),
        None,
    )
    .await;
    assert_eq!(changed.0, 200);
    assert_eq!(changed.1["members"][0]["name"], "MC");
    let stale = http(&app, "GET", &format!("/v1/games/{ID}"), Some(&jwt), None).await;
    assert_eq!(stale.1["members"][0]["name"], "MC");
    let update = http(
        &app,
        "POST",
        &format!("/v1/games/{ID}/display-name"),
        Some(&token(&host)),
        Some(serde_json::json!({"name":"Injected Name", "subject":"someone-else"})),
    )
    .await;
    assert_eq!(update.0, 200);
    assert_eq!(
        h.service.view(ID, &host.sub).await.unwrap().members[0].name,
        "MC"
    );
    let mut visitor = host.clone();
    visitor.sub = "google:visitor".into();
    visitor.name = "Jules M".into();
    visitor.role = Role::Player;
    let command = request(
        h.service.room(ID).await.unwrap().version,
        Command::Join {
            name: "Jules Martin".into(),
            spectator: false,
        },
    );
    let forged = http(
        &app,
        "POST",
        &format!("/v1/games/{ID}/commands"),
        Some(&token(&visitor)),
        Some(serde_json::to_value(command).unwrap()),
    )
    .await;
    assert_eq!(forged.0, 403);
    let old = Grant {
        name_revision: None,
        name: "Maya Louise Chen".into(),
        ..host
    };
    assert_eq!(old.identity().name, "Maya C");
    assert_eq!(
        http(
            &app,
            "POST",
            &format!("/v1/games/{ID}/display-name"),
            Some(&token(&old)),
            Some(serde_json::json!({}))
        )
        .await
        .0,
        403
    );
}
