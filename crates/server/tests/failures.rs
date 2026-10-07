mod common;
use async_trait::async_trait;
use common::*;
use server::{
    error::{Error, Result},
    model::{Record, Room},
    service::GameService,
    storage::{Archive, Store},
};
use settlers_protocol::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
struct FaultStore {
    inner: Arc<dyn Store>,
    fail: AtomicBool,
    after: bool,
}
#[async_trait]
impl Store for FaultStore {
    async fn load(&self, id: &str) -> Result<Option<Room>> {
        self.inner.load(id).await
    }
    async fn ids(&self) -> Result<Vec<String>> {
        self.inner.ids().await
    }
    async fn pending(&self, id: &str) -> Result<Vec<Record>> {
        self.inner.pending(id).await
    }
    async fn prune(&self, id: &str, through: u64) -> Result<()> {
        self.inner.prune(id, through).await
    }
    async fn commit(&self, expected: u64, room: &Room, record: &Record) -> Result<()> {
        if self.fail.swap(false, Ordering::SeqCst) {
            if self.after {
                self.inner.commit(expected, room, record).await?;
            }
            return Err(Error::new(
                ErrorCode::StorageUnavailable,
                "injected failure",
            ));
        }
        self.inner.commit(expected, room, record).await
    }
}
#[tokio::test]
async fn persistence_failure_does_not_mutate_or_broadcast_and_ambiguous_commit_is_reconciled() {
    for after in [false, true] {
        let h = Harness::new().await;
        let store = Arc::new(FaultStore {
            inner: h.store.clone(),
            fail: AtomicBool::new(false),
            after,
        });
        let service = GameService::restore(store.clone(), h.archive.clone(), 1)
            .await
            .unwrap();
        let mut subscriber = service
            .subscribe(&h.id, &identity(0).subject)
            .await
            .unwrap();
        store.fail.store(true, Ordering::SeqCst);
        let req = request(
            1,
            Command::Join {
                name: "A".into(),
                spectator: false,
            },
        );
        let result = service.execute(&h.id, identity(1), req.clone(), 1).await;
        if after {
            assert!(result.is_ok());
            assert_eq!(service.room(&h.id).await.unwrap().version, 2);
            assert!(subscriber.try_recv().is_ok());
        } else {
            assert!(result.is_err());
            assert_eq!(service.room(&h.id).await.unwrap().version, 1);
            assert!(subscriber.try_recv().is_err());
            service.execute(&h.id, identity(1), req, 1).await.unwrap();
        }
    }
}
struct DelayedArchive {
    inner: Arc<dyn Archive>,
    visible: AtomicBool,
}
#[async_trait]
impl Archive for DelayedArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        self.inner.deliver(records).await
    }
    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        if self.visible.load(Ordering::SeqCst) {
            self.inner.read_page(id, after, limit).await
        } else {
            Ok(vec![])
        }
    }
    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        if self.visible.load(Ordering::SeqCst) {
            self.inner.read(id).await
        } else {
            Ok(vec![])
        }
    }
}
#[tokio::test]
async fn delivery_ack_without_archive_visibility_keeps_outbox_and_history() {
    let h = Harness::new().await;
    let archive = Arc::new(DelayedArchive {
        inner: h.archive.clone(),
        visible: AtomicBool::new(false),
    });
    let service = GameService::restore(h.store.clone(), archive.clone(), 1)
        .await
        .unwrap();
    service.flush_archive().await.unwrap();
    assert_eq!(h.record_count().await, 1);
    let page = service
        .history(&h.id, &identity(0).subject, 0, 10)
        .await
        .unwrap();
    assert_eq!(page.next_after, 1);
    archive.visible.store(true, Ordering::SeqCst);
    service.flush_archive().await.unwrap();
    assert_eq!(h.record_count().await, 0);
}

struct FailingGameArchive {
    inner: Arc<dyn Archive>,
    fail_id: String,
}
#[async_trait]
impl Archive for FailingGameArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        if records.iter().any(|r| r.game_id == self.fail_id) {
            return Err(Error::new(ErrorCode::StorageUnavailable, "archive offline"));
        }
        self.inner.deliver(records).await
    }
    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        self.inner.read_page(id, after, limit).await
    }
    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        self.inner.read(id).await
    }
}
#[tokio::test]
async fn archive_outage_does_not_block_restore_or_other_games_delivery() {
    let h = Harness::new().await;
    h.service
        .create(
            identity(0),
            CreateGame {
                research: false,
                name: "Second".into(),
                settings: Settings::default(),
                host_as_player: true,
                player_name: None,
                seed: None,
            },
            0,
        )
        .await
        .unwrap();
    let ids = h.store.ids().await.unwrap();
    let archive = Arc::new(FailingGameArchive {
        inner: h.archive.clone(),
        fail_id: ids[0].clone(),
    });
    let service = GameService::restore(h.store.clone(), archive, 1)
        .await
        .unwrap();
    assert_eq!(service.room(&ids[0]).await.unwrap().version, 1);
    assert_eq!(h.store.pending(&ids[0]).await.unwrap().len(), 1);
    assert!(h.store.pending(&ids[1]).await.unwrap().is_empty());
    assert!(service.flush_archive().await.is_err());
}
#[tokio::test]
async fn failed_restart_presence_commit_is_retried_by_actor() {
    let h = Harness::new().await;
    h.service
        .presence(&h.id, identity(0).subject, "old-socket".into(), true, 1)
        .await
        .unwrap();
    let store = Arc::new(FaultStore {
        inner: h.store.clone(),
        fail: AtomicBool::new(true),
        after: false,
    });
    let service = GameService::restore(store, h.archive.clone(), 2)
        .await
        .unwrap();
    assert!(service.room(&h.id).await.unwrap().members[0].connected);
    service.tick(3).await.unwrap();
    assert!(!service.room(&h.id).await.unwrap().members[0].connected);
}
#[tokio::test]
async fn duplicate_archive_sequence_must_match_input_and_public_events_too() {
    let h = Harness::new().await;
    let record = h.store.pending(&h.id).await.unwrap().remove(0);
    let mut changed = record.clone();
    changed.public.clear();
    assert_ne!(record.public, changed.public);
    assert!(server::storage::merge_records([record, changed]).is_err());
}
