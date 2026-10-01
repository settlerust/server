mod common;
use common::*;
use server::{
    reducer,
    service::GameService,
    storage::{Archive, Store},
};
use settlers_protocol::*;

#[tokio::test]
async fn removed_seats_stay_revoked_after_retries_restart_and_replay() {
    let h = Harness::new().await;
    h.command(
        1,
        Command::Join {
            name: "Newcomer".into(),
            spectator: false,
        },
    )
    .await;
    let update = AdmissionUpdate {
        computers: vec![],
        revision: 3,
        players: vec![identity(0).subject, identity(2).subject],
    };
    h.service
        .admission(&h.id, identity(0), update.clone(), 2)
        .await
        .unwrap();
    let version = h.room().await.version;
    h.service
        .admission(&h.id, identity(0), update.clone(), 3)
        .await
        .unwrap();
    assert_eq!(h.room().await.version, version);
    assert!(h.room().await.member(&identity(1).subject).is_none());
    let stale = AdmissionUpdate {
        computers: vec![],
        revision: 2,
        players: vec![identity(0).subject, identity(1).subject],
    };
    assert_eq!(
        h.service
            .admission(&h.id, identity(0), stale, 4)
            .await
            .unwrap_err()
            .0
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        h.service
            .admission(&h.id, identity(1), update.clone(), 4)
            .await
            .unwrap_err()
            .0
            .code,
        ErrorCode::Forbidden
    );
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), 5)
        .await
        .unwrap();
    let join = request(
        restored.room(&h.id).await.unwrap().version,
        Command::JoinAgent {
            name: "Newcomer".into(),
        },
    );
    assert_eq!(
        restored
            .execute(&h.id, identity(1), join, 6)
            .await
            .unwrap_err()
            .0
            .code,
        ErrorCode::Forbidden
    );
    restored.flush_archive().await.unwrap();
    assert!(h.store.pending(&h.id).await.unwrap().is_empty());
    let replay = reducer::replay(&h.archive.read(&h.id).await.unwrap()).unwrap();
    assert_eq!(replay.admission, Some(update));
    assert!(replay.member(&identity(1).subject).is_none());
}

#[tokio::test]
async fn admission_cannot_evict_a_started_game_and_removed_opening_is_reset() {
    let h = Harness::new().await;
    for player in 1..4 {
        h.command(
            player,
            Command::Join {
                name: format!("P{player}"),
                spectator: false,
            },
        )
        .await;
    }
    h.ready_all().await;
    h.command(0, Command::RollForOrder).await;
    assert!(h.room().await.opening.is_some());
    h.service
        .admission(
            &h.id,
            identity(0),
            AdmissionUpdate {
                computers: vec![],
                revision: 1,
                players: (0..3).map(|p| identity(p).subject).collect(),
            },
            2,
        )
        .await
        .unwrap();
    assert!(h.room().await.opening.is_none());
    h.service
        .admission(
            &h.id,
            identity(0),
            AdmissionUpdate {
                computers: vec![],
                revision: 2,
                players: (0..4).map(|p| identity(p).subject).collect(),
            },
            3,
        )
        .await
        .unwrap();
    h.command(
        3,
        Command::Join {
            name: "Returning".into(),
            spectator: false,
        },
    )
    .await;
    h.ready_all().await;
    h.command(0, Command::Start).await;
    assert_eq!(
        h.service
            .admission(
                &h.id,
                identity(0),
                AdmissionUpdate {
                    computers: vec![],
                    revision: 3,
                    players: vec![identity(0).subject]
                },
                4
            )
            .await
            .unwrap_err()
            .0
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(h.room().await.members.len(), 4);
}

#[cfg(feature = "aws")]
#[tokio::test]
async fn research_rows_preserve_replay_payload_and_route_all_parts_of_a_game_together() {
    let h = Harness::new().await;
    let records = h.store.pending(&h.id).await.unwrap();
    let row: serde_json::Value =
        serde_json::from_slice(&server::storage::research::encode(&records[0]).unwrap()).unwrap();
    let replay: server::model::Record =
        serde_json::from_str(row["payload_json"].as_str().unwrap()).unwrap();
    assert_eq!(replay, records[0]);
    assert_eq!(row["event_id"], format!("{}:1", h.id));
    let mut records = records;
    for sequence in 2..1001 {
        let mut record = records[0].clone();
        record.sequence = sequence;
        records.push(record);
    }
    let grouped =
        server::storage::firehose_batches("settlers-a,settlers-b,settlers-c,settlers-d", &records);
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped.values().next().unwrap().len(), 1000);
}
