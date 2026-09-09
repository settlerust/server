mod common;
use common::identity;
use server::{
    service::GameService,
    storage::{
        Store,
        local::{LocalArchive, LocalStore},
    },
};
use settlers_protocol::{CreateGame, ErrorCode, Settings};
use std::sync::Arc;

#[tokio::test]
async fn concurrent_creation_cannot_exceed_capacity_and_existing_retries_still_work() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(LocalStore::open(&dir.path().join("state.sqlite3")).unwrap());
    let archive = Arc::new(LocalArchive::new(dir.path().join("archive")));
    let service = GameService::restore_with_capacity(store.clone(), archive, 0, 1)
        .await
        .unwrap();
    let request = CreateGame {
        research: false,
        name: "Capacity".into(),
        settings: Settings::default(),
        host_as_player: true,
        player_name: None,
    };
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        service.create_assigned(&first, identity(0), request.clone(), 0),
        service.create_assigned(&second, identity(0), request.clone(), 0),
    );
    let (winner, rejected) = match (a, b) {
        (Ok(game), Err(error)) | (Err(error), Ok(game)) => (game, error),
        _ => panic!("Exactly one concurrent creation must fit"),
    };
    assert_eq!(rejected.0.code, ErrorCode::Backpressure);
    assert_eq!(service.loaded_games().await, 1);
    assert_eq!(store.ids().await.unwrap(), vec![winner.id.clone()]);
    let retry = service
        .create_assigned(&winner.id, identity(0), request, 1)
        .await
        .unwrap();
    assert_eq!(retry.version, winner.version);
}
