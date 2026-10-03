#![cfg(feature = "aws")]
mod common;
#[path = "support/s3.rs"]
mod s3;
use common::*;
use server::{
    model::Record,
    reducer,
    service::GameService,
    storage::{Archive, Store, aws::FirehoseArchive},
};
use settlers_protocol::*;
use std::sync::Arc;

fn records(id: &str, sequences: impl IntoIterator<Item = u64>) -> Vec<Record> {
    let (_, template) = reducer::create(
        id.into(),
        CreateGame {
            research: false,
            name: "S3 fixture".into(),
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
    sequences
        .into_iter()
        .map(|sequence| {
            let mut record = template.clone();
            record.sequence = sequence;
            record
        })
        .collect()
}

#[tokio::test]
async fn batch_pages_fetch_only_requested_ranges_and_retries_do_not_rewrite() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, 1..=320);
    archive.deliver(&rows).await.unwrap();
    assert_eq!(fixture.state.lock().unwrap().puts, 4);
    archive.deliver(&rows).await.unwrap();
    assert_eq!(fixture.state.lock().unwrap().puts, 4);
    {
        let mut state = fixture.state.lock().unwrap();
        state.calls.clear();
        state.objects.get_mut(&s3::key(&id, 1)).unwrap().bytes = b"corrupt old batch".to_vec();
    }
    let page = archive.read_page(&id, 295, 10).await.unwrap();
    assert_eq!(
        page.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        (296..=305).collect::<Vec<_>>()
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|(_, key)| key.clone())
            .collect::<Vec<_>>(),
        vec![s3::key(&id, 201), s3::key(&id, 301)]
    );
    assert_eq!(
        archive.read_page(&id, 300, 10).await.unwrap(),
        rows[300..310]
    );
    let calls = fixture.state.lock().unwrap().calls.len();
    assert!(archive.read_page(&id, 0, 0).await.unwrap().is_empty());
    assert!(
        archive
            .read_page(&id, u64::MAX, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.state.lock().unwrap().calls.len(), calls);
    assert_eq!(fixture.state.lock().unwrap().lists, 0);
}

#[tokio::test]
async fn partial_batches_merge_overlapping_workers_without_losing_records() {
    let fixture = s3::Fixture::new().await;
    fixture.state.lock().unwrap().barrier = Some(Arc::new(tokio::sync::Barrier::new(2)));
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, 1..=100);
    let (left, right) = tokio::join!(archive.deliver(&rows[..70]), archive.deliver(&rows[40..]));
    left.unwrap();
    right.unwrap();
    assert_eq!(archive.read_page(&id, 0, 100).await.unwrap(), rows);
    assert_eq!(fixture.state.lock().unwrap().puts, 3);
    let mut conflict = rows[40].clone();
    conflict.timestamp_ms += 1;
    assert!(archive.deliver(&[conflict]).await.is_err());
    assert_eq!(archive.read_page(&id, 0, 100).await.unwrap(), rows);
    assert_eq!(fixture.state.lock().unwrap().puts, 3);
}

#[tokio::test]
async fn archive_readback_failure_keeps_outbox_and_snapshot_recoverable() {
    let h = Harness::new().await;
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    fixture.state.lock().unwrap().fail_get = Some(2);
    let service = GameService::restore(h.store.clone(), archive.clone(), 0)
        .await
        .unwrap();
    assert_eq!(h.record_count().await, 1);
    fixture.state.lock().unwrap().fail_get = Some(3);
    assert!(service.flush_archive().await.is_err());
    assert_eq!(h.store.pending(&h.id).await.unwrap().len(), 1);
    assert_eq!(h.store.load(&h.id).await.unwrap().unwrap().version, 1);
    let recovered = GameService::restore(h.store.clone(), archive.clone(), 0)
        .await
        .unwrap();
    recovered.flush_archive().await.unwrap();
    assert!(h.store.pending(&h.id).await.unwrap().is_empty());
    assert_eq!(fixture.state.lock().unwrap().puts, 1);
    assert_eq!(
        recovered
            .history(&h.id, &identity(0).subject, 0, 100)
            .await
            .unwrap()
            .next_after,
        1
    );
}

#[tokio::test]
async fn lost_s3_write_acknowledgment_retries_identically_and_prunes_only_after_confirmation() {
    let h = Harness::new().await;
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    fixture.state.lock().unwrap().fail_put_after_write = true;
    let service = GameService::restore(h.store.clone(), archive, 0)
        .await
        .unwrap();
    assert!(!fixture.state.lock().unwrap().fail_put_after_write);
    assert_eq!(h.record_count().await, 1);
    service.flush_archive().await.unwrap();
    assert_eq!(h.record_count().await, 0);
    assert_eq!(fixture.state.lock().unwrap().puts, 1);
}

#[tokio::test]
async fn conditional_conflicts_are_bounded_and_outbox_retries_can_finish() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, 1..=3);
    fixture.state.lock().unwrap().conflict_puts = 6;
    assert!(archive.deliver(&rows).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().puts, 6);
    assert!(fixture.state.lock().unwrap().objects.is_empty());
    archive.deliver(&rows).await.unwrap();
    assert_eq!(archive.read_page(&id, 0, 100).await.unwrap(), rows);
}

#[tokio::test]
async fn partial_multi_batch_failure_retries_without_rewriting_confirmed_batches() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, 1..=150);
    fixture.state.lock().unwrap().fail_put = Some(2);
    assert!(archive.deliver(&rows).await.is_err());
    assert_eq!(archive.read_page(&id, 0, 150).await.unwrap(), rows[..100]);
    archive.deliver(&rows).await.unwrap();
    assert_eq!(archive.read_page(&id, 0, 150).await.unwrap(), rows);
    assert_eq!(fixture.state.lock().unwrap().puts, 3);
}

#[tokio::test]
async fn public_history_stops_at_s3_gaps_and_merges_the_pending_tail() {
    let h = Harness::new().await;
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let service = GameService::restore(h.store.clone(), archive.clone(), 0)
        .await
        .unwrap();
    for _ in 0..3 {
        let version = service.room(&h.id).await.unwrap().version;
        service
            .execute(
                &h.id,
                identity(0),
                request(version, Command::SetReady { ready: false }),
                1,
            )
            .await
            .unwrap();
    }
    service.flush_archive().await.unwrap();
    let mut rows = archive.read_page(&h.id, 0, 100).await.unwrap();
    rows.remove(1);
    fixture
        .state
        .lock()
        .unwrap()
        .objects
        .get_mut(&s3::key(&h.id, 1))
        .unwrap()
        .bytes = s3::gzip(&rows);
    let page = service
        .history(&h.id, &identity(0).subject, 0, 100)
        .await
        .unwrap();
    assert_eq!(page.next_after, 1);
    assert!(page.archive_pending);
    let version = service.room(&h.id).await.unwrap().version;
    service
        .execute(
            &h.id,
            identity(0),
            request(version, Command::SetReady { ready: false }),
            1,
        )
        .await
        .unwrap();
    let tail = service
        .history(&h.id, &identity(0).subject, 4, 100)
        .await
        .unwrap();
    assert_eq!(tail.next_after, 5);
    assert!(!tail.archive_pending);
    assert_eq!(fixture.state.lock().unwrap().lists, 0);
}

#[tokio::test]
async fn gaps_caps_and_last_u64_sequence_keep_read_work_bounded() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, [1, 3, 100, 101, 256, 257, u64::MAX]);
    archive.deliver(&rows).await.unwrap();
    assert_eq!(
        archive.read_page(&id, 0, usize::MAX).await.unwrap(),
        rows[..5]
    );
    let calls = fixture.state.lock().unwrap().calls.len();
    archive.read_page(&id, 99, usize::MAX).await.unwrap();
    assert_eq!(fixture.state.lock().unwrap().calls.len() - calls, 4);
    assert_eq!(
        archive.read_page(&id, u64::MAX - 1, 100).await.unwrap(),
        rows[6..]
    );
    assert!(archive.read_page(&id, 1000, 100).await.unwrap().is_empty());
    assert!(archive.read_page("../escape", 0, 1).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().lists, 0);
}

#[tokio::test]
async fn malformed_inputs_and_corrupt_batches_fail_before_overwriting_history() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, 1..=2);
    for field in 0..4 {
        let mut invalid = rows[1].clone();
        match field {
            0 => invalid.sequence = 0,
            1 => invalid.schema = 2,
            2 => invalid.game_id = "../escape".into(),
            _ => invalid.state_hash = "x".repeat(100_001),
        }
        assert!(archive.deliver(&[rows[0].clone(), invalid]).await.is_err());
    }
    assert_eq!(fixture.state.lock().unwrap().puts, 0);
    archive.deliver(&rows).await.unwrap();
    for invalid in [
        records(&id, [101]),
        records(&uuid::Uuid::new_v4().to_string(), [1]),
        records(&id, [2, 1]),
        records(&id, [1, 1]),
    ] {
        fixture
            .state
            .lock()
            .unwrap()
            .objects
            .get_mut(&s3::key(&id, 1))
            .unwrap()
            .bytes = s3::gzip(&invalid);
        assert!(archive.read_page(&id, 0, 2).await.is_err());
        assert!(archive.deliver(&rows).await.is_err());
    }
    fixture
        .state
        .lock()
        .unwrap()
        .objects
        .get_mut(&s3::key(&id, 1))
        .unwrap()
        .bytes = b"invalid gzip".to_vec();
    assert!(archive.read_page(&id, 0, 2).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().puts, 1);
}

#[tokio::test]
async fn compressed_and_decoded_size_limits_reject_unbounded_objects() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let id = uuid::Uuid::new_v4().to_string();
    let rows = records(&id, [1]);
    archive.deliver(&rows).await.unwrap();
    let replace = |bytes| {
        fixture
            .state
            .lock()
            .unwrap()
            .objects
            .get_mut(&s3::key(&id, 1))
            .unwrap()
            .bytes = bytes;
    };
    replace(vec![0; 10_065_637]);
    assert!(archive.read_page(&id, 0, 1).await.is_err());
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&vec![b' '; 10_000_101]).unwrap();
    replace(encoder.finish().unwrap());
    assert!(archive.read_page(&id, 0, 1).await.is_err());
    replace(s3::gzip(&[]));
    assert!(archive.read_page(&id, 0, 1).await.is_err());
    let mut truncated = s3::gzip(&rows);
    truncated.truncate(truncated.len() - 4);
    replace(truncated);
    assert!(archive.read_page(&id, 0, 1).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().puts, 1);
}

#[tokio::test]
async fn verified_fixture_import_and_full_export_preserve_base_v9_replay() {
    let fixture = s3::Fixture::new().await;
    let archive = fixture.archive();
    let rows: Vec<Record> = include_str!("fixtures/base-v9-game.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let id = &rows[0].game_id;
    fixture.state.lock().unwrap().objects.insert(
        format!("events/game_id={id}/slice.gz"),
        s3::Object {
            bytes: s3::gzip(&rows),
            etag: "\"export\"".into(),
        },
    );
    let imported = archive.read_firehose_export(id).await.unwrap();
    reducer::replay(&imported).unwrap();
    archive.deliver(&imported).await.unwrap();
    archive.deliver(&imported).await.unwrap();
    let exported = archive.read(id).await.unwrap();
    assert_eq!(exported, rows);
    assert_eq!(
        reducer::state_hash(&reducer::replay(&exported).unwrap()).unwrap(),
        "2a2cfbbeafb9c0937548643602cc4a00d113597892ee72d14bebe988289acc8e"
    );
    assert_eq!(fixture.state.lock().unwrap().puts, 1);
    assert!(fixture.state.lock().unwrap().firehose.is_empty());
}

#[tokio::test]
async fn optional_firehose_partial_failure_keeps_s3_and_outbox_untouched() {
    let h = Harness::new().await;
    let fixture = s3::Fixture::new().await;
    let archive = Arc::new(FirehoseArchive::new(
        fixture.archive(),
        fixture.firehose.clone(),
        "test-stream".into(),
    ));
    fixture.state.lock().unwrap().fail_firehose = true;
    let service = GameService::restore(h.store.clone(), archive.clone(), 0)
        .await
        .unwrap();
    assert!(!fixture.state.lock().unwrap().fail_firehose);
    assert_eq!(h.record_count().await, 1);
    assert!(fixture.state.lock().unwrap().objects.is_empty());
    service.flush_archive().await.unwrap();
    assert_eq!(h.record_count().await, 0);
    assert_eq!(fixture.state.lock().unwrap().puts, 1);
    use base64::Engine;
    let state = fixture.state.lock().unwrap();
    let payload = base64::engine::general_purpose::STANDARD
        .decode(state.firehose[1]["Records"][0]["Data"].as_str().unwrap())
        .unwrap();
    assert!(payload.ends_with(b"\n"));
    let row: Record = serde_json::from_slice(&payload).unwrap();
    assert_eq!(row.game_id, h.id);
}
