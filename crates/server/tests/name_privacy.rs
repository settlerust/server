mod common;
use common::*;
use server::{
    auth::Identity,
    reducer,
    service::GameService,
    storage::{Archive, Store},
};
use settlers_protocol::*;

#[tokio::test]
async fn current_privacy_redacts_snapshots_live_batches_and_archived_history_after_restart() {
    let h = Harness::new().await;
    let person = Identity {
        subject: "google:private-subject".into(),
        name: "Maya Louise Chen".into(),
    };
    h.service
        .execute(
            &h.id,
            person.clone(),
            request(
                1,
                Command::Join {
                    name: person.name.clone(),
                    spectator: false,
                },
            ),
            1,
        )
        .await
        .unwrap();
    let legacy = h.service.view(&h.id, "player-0").await.unwrap();
    assert_eq!(legacy.members[1].name, "Maya C");
    let history = h.service.history(&h.id, "player-0", 0, 100).await.unwrap();
    assert!(!serde_json::to_string(&history).unwrap().contains("Louise"));

    // Full name is visible only after a server-signed profile revision opts in.
    h.service
        .display_name(&h.id, person.clone(), 1, 2)
        .await
        .unwrap();
    assert_eq!(
        h.service.view(&h.id, "player-0").await.unwrap().members[1].name,
        person.name
    );
    let private = Identity {
        name: "MC".into(),
        ..person.clone()
    };
    h.service
        .display_name(&h.id, private.clone(), 2, 3)
        .await
        .unwrap();
    let version = h.room().await.version;
    // A still-valid old token and an equal-revision conflicting token cannot undo privacy.
    h.service
        .display_name(&h.id, person.clone(), 1, 4)
        .await
        .unwrap();
    h.service
        .display_name(&h.id, person.clone(), 2, 4)
        .await
        .unwrap();
    assert_eq!(h.room().await.version, version);
    assert_eq!(
        h.service.view(&h.id, "player-0").await.unwrap().members[1].name,
        "MC"
    );
    for record in h.store.pending(&h.id).await.unwrap() {
        let batch = reducer::batch(&h.room().await, "player-0", &record).unwrap();
        assert!(!serde_json::to_string(&batch).unwrap().contains("Maya"));
    }
    h.service.flush_archive().await.unwrap();
    let replay = reducer::replay(&h.archive.read(&h.id).await.unwrap()).unwrap();
    assert_eq!(replay.display_names[&person.subject].name, "MC");
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), 5)
        .await
        .unwrap();
    let history = restored.history(&h.id, "player-0", 0, 100).await.unwrap();
    let json = serde_json::to_string(&history).unwrap();
    assert!(json.contains("MC"));
    assert!(!json.contains("Maya"));
    assert_eq!(
        restored.view(&h.id, "player-0").await.unwrap().members[1].name,
        "MC"
    );

    // Leaving a table must not freeze the former participant's history at full name.
    restored
        .admission(
            &h.id,
            identity(0),
            AdmissionUpdate {
                revision: 1,
                players: vec!["player-0".into()],
            },
            6,
        )
        .await
        .unwrap();
    restored
        .display_name(
            &h.id,
            Identity {
                name: "M".into(),
                ..private
            },
            3,
            7,
        )
        .await
        .unwrap();
    assert_eq!(
        restored.room(&h.id).await.unwrap().display_names[&person.subject].name,
        "M"
    );
    let history = restored.history(&h.id, "player-0", 0, 100).await.unwrap();
    assert!(!serde_json::to_string(&history).unwrap().contains("MC"));
}
