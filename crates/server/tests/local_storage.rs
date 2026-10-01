mod common;

use common::*;
use server::{
    model::{Input, Record, Room},
    reducer,
    storage::{Archive, OUTBOX_LIMIT, Store, local::LocalArchive, merge_records},
};
use settlers_protocol::ErrorCode;

fn next_presence(room: &Room, connected: bool) -> (Room, Record) {
    reducer::reduce(
        room,
        Input::Presence {
            subject: identity(0).subject,
            connected,
        },
        room.version,
    )
    .unwrap()
}

#[tokio::test]
async fn stale_commit_preserves_both_snapshot_and_outbox() {
    let h = Harness::new().await;
    let before = h.room().await;
    let pending = h.store.pending(&h.id).await.unwrap();
    let (next, record) = next_presence(&before, true);

    let error = h.store.commit(0, &next, &record).await.unwrap_err();
    assert_eq!(error.0.code, ErrorCode::Conflict);
    assert_eq!(error.0.current_version, Some(before.version));
    let stored = h.store.load(&h.id).await.unwrap().unwrap();
    assert_eq!(
        reducer::state_hash(&stored).unwrap(),
        reducer::state_hash(&before).unwrap()
    );
    assert_eq!(h.store.pending(&h.id).await.unwrap(), pending);
}

#[tokio::test]
async fn outbox_insert_failure_rolls_back_the_snapshot_write() {
    let h = Harness::new().await;
    let before = h.room().await;
    let pending = h.store.pending(&h.id).await.unwrap();
    let mut changed = before.clone();
    changed.name = "This update must roll back".into();

    // Reusing the sequence violates the outbox primary key after the snapshot write.
    let error = h
        .store
        .commit(before.version, &changed, &pending[0])
        .await
        .unwrap_err();
    assert_eq!(error.0.code, ErrorCode::StorageUnavailable);
    let stored = h.store.load(&h.id).await.unwrap().unwrap();
    assert_eq!(
        reducer::state_hash(&stored).unwrap(),
        reducer::state_hash(&before).unwrap()
    );
    assert_eq!(h.store.pending(&h.id).await.unwrap(), pending);
}

#[tokio::test]
async fn full_outbox_pauses_writes_and_pruning_restores_capacity() {
    let h = Harness::new().await;
    let mut room = h.room().await;
    for _ in 1..OUTBOX_LIMIT {
        let (next, record) = next_presence(&room, !room.members[0].connected);
        h.store.commit(room.version, &next, &record).await.unwrap();
        room = next;
    }
    let pending = h.store.pending(&h.id).await.unwrap();
    assert_eq!(pending.len(), OUTBOX_LIMIT);
    let (next, record) = next_presence(&room, !room.members[0].connected);
    let error = h
        .store
        .commit(room.version, &next, &record)
        .await
        .unwrap_err();
    assert_eq!(error.0.code, ErrorCode::Backpressure);
    assert_eq!(
        h.store.load(&h.id).await.unwrap().unwrap().version,
        room.version
    );
    assert_eq!(h.store.pending(&h.id).await.unwrap(), pending);

    h.store.prune(&h.id, 1).await.unwrap();
    h.store.commit(room.version, &next, &record).await.unwrap();
    assert_eq!(
        h.store.load(&h.id).await.unwrap().unwrap().version,
        next.version
    );
    let pending = h.store.pending(&h.id).await.unwrap();
    assert_eq!(pending.len(), OUTBOX_LIMIT);
    assert_eq!(pending.first().unwrap().sequence, 2);
    assert_eq!(pending.last().unwrap(), &record);
}

#[tokio::test]
async fn archive_retries_accept_only_identical_records() {
    let h = Harness::new().await;
    let record = h.store.pending(&h.id).await.unwrap().remove(0);
    h.archive
        .deliver(std::slice::from_ref(&record))
        .await
        .unwrap();
    h.archive
        .deliver(std::slice::from_ref(&record))
        .await
        .unwrap();
    assert_eq!(h.archive.read(&h.id).await.unwrap(), vec![record.clone()]);

    let mut different_events = record.clone();
    different_events.public.clear();
    let mut different_input = record.clone();
    different_input.input = Input::Timeout;
    let mut different_timestamp = record.clone();
    different_timestamp.timestamp_ms += 1;
    let mut different_version = record.clone();
    different_version.engine_version = "base-v1".into();
    let mut different_hash = record.clone();
    different_hash.state_hash = "different".into();

    for changed in [
        different_events,
        different_input,
        different_timestamp,
        different_version,
        different_hash,
    ] {
        assert_ne!(changed, record);
        let error = h.archive.deliver(&[changed]).await.unwrap_err();
        assert_eq!(error.0.code, ErrorCode::Internal);
        assert_eq!(h.archive.read(&h.id).await.unwrap(), vec![record.clone()]);
    }
}

#[tokio::test]
async fn corrupt_archive_record_is_reported_and_never_overwritten() {
    let h = Harness::new().await;
    let record = h.store.pending(&h.id).await.unwrap().remove(0);
    let dir = h
        .dir
        .path()
        .join("archive")
        .join(format!("game_id={}", h.id));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{:020}.json", record.sequence));
    std::fs::write(&path, b"corrupt record").unwrap();

    assert!(h.archive.deliver(&[record]).await.is_err());
    assert!(h.archive.read(&h.id).await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"corrupt record");
}

#[tokio::test]
async fn archive_rejects_non_uuid_paths_before_creating_files() {
    let h = Harness::new().await;
    let root = h.dir.path().join("isolated-archive");
    let archive = LocalArchive::new(root.clone());
    let record = h.store.pending(&h.id).await.unwrap().remove(0);
    for id in ["../outside", "", "/tmp/outside", "game_id=not-a-uuid"] {
        let mut record = record.clone();
        record.game_id = id.into();
        assert_eq!(
            archive.deliver(&[record]).await.unwrap_err().0.code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            archive.read(id).await.unwrap_err().0.code,
            ErrorCode::InvalidRequest
        );
    }
    assert!(!root.exists());
}

#[tokio::test]
async fn merged_history_sorts_sequences_and_deduplicates_exact_retries() {
    let h = Harness::new().await;
    let first = h.store.pending(&h.id).await.unwrap().remove(0);
    let (_, second) = next_presence(&h.room().await, true);
    let merged =
        merge_records([second.clone(), first.clone(), second.clone(), first.clone()]).unwrap();
    assert_eq!(merged, vec![first, second]);
}
