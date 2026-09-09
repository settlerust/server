mod common;
use common::*;
use server::{
    reducer,
    service::GameService,
    storage::{Archive, Store},
};
use settlers_protocol::*;
#[tokio::test]
async fn acknowledged_requests_are_idempotent_across_restart_and_cannot_be_repurposed() {
    let h = Harness::new().await;
    let req = request(
        1,
        Command::Join {
            name: "B".into(),
            spectator: false,
        },
    );
    let first = h
        .service
        .execute(&h.id, identity(1), req.clone(), 1)
        .await
        .unwrap();
    let service = GameService::restore(h.store.clone(), h.archive.clone(), 2)
        .await
        .unwrap();
    let duplicate = service
        .execute(&h.id, identity(1), req.clone(), 2)
        .await
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.version, first.version);
    let mut wrong = req;
    wrong.command = Command::Start;
    assert_eq!(
        service
            .execute(&h.id, identity(1), wrong, 2)
            .await
            .unwrap_err()
            .0
            .code,
        ErrorCode::InvalidRequest
    );
}
#[tokio::test]
async fn archive_pruning_preserves_paginated_history_and_verified_replay() {
    let h = Harness::new().await;
    h.start().await;
    for _ in 0..12 {
        let room = h.room().await;
        let next = room.deadline_ms.unwrap();
        h.service.tick(next).await.unwrap();
    }
    let expected = h.room().await;
    assert!(h.record_count().await > 0);
    h.service.flush_archive().await.unwrap();
    assert_eq!(h.record_count().await, 0);
    let records = h.archive.read(&h.id).await.unwrap();
    assert_eq!(
        reducer::state_hash(&reducer::replay(&records).unwrap()).unwrap(),
        reducer::state_hash(&expected).unwrap()
    );
    let mut after = 0;
    let mut sequences = vec![];
    loop {
        let page = h
            .service
            .history(&h.id, &identity(0).subject, after, 3)
            .await
            .unwrap();
        sequences.extend(page.events.iter().map(|b| b.sequence));
        if page.next_after == expected.version {
            break;
        }
        assert!(page.next_after > after);
        after = page.next_after;
    }
    assert_eq!(sequences, (1..=expected.version).collect::<Vec<_>>());
}
#[tokio::test]
async fn stale_writes_do_not_replace_state_or_append_events() {
    let h = Harness::new().await;
    let room = h.room().await;
    let (a, ra) = reducer::reduce(
        &room,
        server::model::Input::Command {
            identity: identity(1),
            request: request(
                1,
                Command::Join {
                    name: "A".into(),
                    spectator: false,
                },
            ),
        },
        1,
    )
    .unwrap();
    let (b, rb) = reducer::reduce(
        &room,
        server::model::Input::Command {
            identity: identity(2),
            request: request(
                1,
                Command::Join {
                    name: "B".into(),
                    spectator: false,
                },
            ),
        },
        1,
    )
    .unwrap();
    let (first, second) = tokio::join!(h.store.commit(1, &a, &ra), h.store.commit(1, &b, &rb));
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(h.record_count().await, 2);
}
#[tokio::test]
async fn disconnect_grants_one_full_allotment_and_reconnect_does_not_reset_repeatedly() {
    let h = Harness::new().await;
    h.start().await;
    let before = h.room().await;
    let duration = before.settings.turn_seconds as u64 * 1000;
    let current = before.game.unwrap().current;
    h.service
        .presence(
            &h.id,
            identity(current).subject,
            format!("test-{current}"),
            true,
            1000,
        )
        .await
        .unwrap();
    h.service
        .presence(
            &h.id,
            identity(current).subject,
            format!("test-{current}"),
            false,
            2000,
        )
        .await
        .unwrap();
    let once = h.room().await;
    assert_eq!(once.deadline_ms, Some(2000 + duration));
    assert!(!once.member(&identity(current).subject).unwrap().connected);
    h.service
        .presence(
            &h.id,
            identity(current).subject,
            "socket-b".into(),
            true,
            3000,
        )
        .await
        .unwrap();
    h.service
        .presence(
            &h.id,
            identity(current).subject,
            "socket-b".into(),
            false,
            4000,
        )
        .await
        .unwrap();
    assert_eq!(h.room().await.deadline_ms, once.deadline_ms);
    h.service.tick(once.deadline_ms.unwrap()).await.unwrap();
    assert_ne!(
        h.room().await.game.unwrap().phase,
        before.game.unwrap().phase
    );
}
#[tokio::test]
async fn closing_one_of_two_sockets_does_not_mark_player_disconnected() {
    let h = Harness::new().await;
    for session in ["a", "b"] {
        h.service
            .presence(&h.id, identity(0).subject, session.into(), true, 1)
            .await
            .unwrap();
    }
    h.service
        .presence(&h.id, identity(0).subject, "a".into(), false, 2)
        .await
        .unwrap();
    assert!(
        h.room()
            .await
            .member(&identity(0).subject)
            .unwrap()
            .connected
    );
}

#[tokio::test]
async fn host_can_configure_lobby_but_started_rules_are_fixed() {
    let h = Harness::new().await;
    h.command(
        0,
        Command::Configure {
            settings: Settings {
                players: 2,
                points_to_win: 5,
                discard_limit: 8,
                turn_seconds: 30,
                deterministic: false,
            },
        },
    )
    .await;
    h.command(
        1,
        Command::Join {
            name: "B".into(),
            spectator: false,
        },
    )
    .await;
    let version = h.room().await.version;
    let response = h
        .service
        .execute(
            &h.id,
            identity(1),
            request(
                version,
                Command::Configure {
                    settings: Settings::default(),
                },
            ),
            1,
        )
        .await;
    assert_eq!(response.unwrap_err().0.code, ErrorCode::Forbidden);
    h.ready_all().await;
    h.command(0, Command::Start).await;
    let version = h.room().await.version;
    assert!(
        h.service
            .execute(
                &h.id,
                identity(0),
                request(
                    version,
                    Command::Configure {
                        settings: Settings::default()
                    }
                ),
                1
            )
            .await
            .is_err()
    );
}
