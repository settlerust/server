mod common;
use async_trait::async_trait;
use common::*;
use server::{
    error::Result,
    model::Record,
    service::GameService,
    storage::{Archive, Store, local::LocalArchive, sequence_window},
};
use settlers_protocol::*;
use std::sync::{Arc, Mutex};
struct PagedOnly {
    archive: Arc<LocalArchive>,
    calls: Mutex<Vec<(u64, usize)>>,
}
#[async_trait]
impl Archive for PagedOnly {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        self.archive.deliver(records).await
    }
    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        self.calls.lock().unwrap().push((after, limit));
        self.archive.read_page(id, after, limit).await
    }
    async fn read(&self, _: &str) -> Result<Vec<Record>> {
        panic!("online paths must not read whole archives")
    }
}
#[tokio::test]
async fn long_history_and_pruning_use_only_bounded_sequence_reads() {
    let h = Harness::new().await;
    let archive = Arc::new(PagedOnly {
        archive: h.archive.clone(),
        calls: Mutex::new(vec![]),
    });
    let service = GameService::restore(h.store.clone(), archive.clone(), 0)
        .await
        .unwrap();
    for _ in 0..320 {
        let room = service.room(&h.id).await.unwrap();
        service
            .execute(
                &h.id,
                identity(0),
                request(room.version, Command::SetReady { ready: false }),
                1,
            )
            .await
            .unwrap();
        if room.version % 50 == 0 {
            service.flush_archive().await.unwrap();
        }
    }
    service.flush_archive().await.unwrap();
    assert_eq!(h.record_count().await, 0);
    assert!(
        archive
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, limit)| *limit <= 256)
    );
    archive.calls.lock().unwrap().clear();
    let dir = h.dir.path().join(format!("archive/game_id={}", h.id));
    std::fs::write(dir.join(format!("{:020}.json", 1)), b"corrupt old file").unwrap();
    let page = service
        .history(&h.id, &identity(0).subject, 250, 10)
        .await
        .unwrap();
    assert_eq!(
        page.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        (251..=260).collect::<Vec<_>>()
    );
    assert_eq!(*archive.calls.lock().unwrap(), vec![(250, 10)]);
    assert!(!page.archive_pending);
    std::fs::remove_file(dir.join(format!("{:020}.json", 251))).unwrap();
    let gap = service
        .history(&h.id, &identity(0).subject, 250, 10)
        .await
        .unwrap();
    assert!(gap.events.is_empty());
    assert_eq!(gap.next_after, 250);
    assert!(gap.archive_pending);
    let tail = service
        .history(&h.id, &identity(0).subject, 320, 100)
        .await
        .unwrap();
    assert_eq!(tail.events.len(), 1);
    assert_eq!(tail.next_after, 321);
    let empty = service
        .history(&h.id, &identity(0).subject, 321, 100)
        .await
        .unwrap();
    assert!(empty.events.is_empty());
    assert!(!empty.archive_pending);
    assert!(
        service
            .history(&h.id, &identity(0).subject, 322, 1)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn local_pages_keep_gaps_and_verify_requested_record_keys() {
    let h = Harness::new().await;
    let mut record = h.store.pending(&h.id).await.unwrap().remove(0);
    record.sequence = 1000;
    h.archive.deliver(&[record.clone()]).await.unwrap();
    let page = h.archive.read_page(&h.id, 999, 1).await.unwrap();
    assert_eq!(page, vec![record.clone()]);
    assert!(h.archive.read_page(&h.id, 0, 256).await.unwrap().is_empty());
    assert!(h.archive.read_page(&h.id, 999, 0).await.unwrap().is_empty());
    assert!(
        h.archive
            .read_page(&h.id, u64::MAX, 100)
            .await
            .unwrap()
            .is_empty()
    );
    let path = h
        .dir
        .path()
        .join(format!("archive/game_id={}/{:020}.json", h.id, 1000));
    record.sequence = 999;
    std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(h.archive.read_page(&h.id, 999, 1).await.is_err());
    assert!(h.archive.read_page("../escape", 0, 1).await.is_err());
}
#[test]
fn sequence_windows_handle_zero_caps_and_cursor_overflow() {
    assert_eq!(sequence_window(0, 0), None);
    assert_eq!(sequence_window(u64::MAX, 1), None);
    assert_eq!(
        sequence_window(u64::MAX - 1, 100),
        Some((u64::MAX, u64::MAX))
    );
    assert_eq!(sequence_window(1000, usize::MAX), Some((1001, 1256)));
}
