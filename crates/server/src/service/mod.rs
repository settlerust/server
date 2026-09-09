mod actor;
mod archive;
use crate::{
    auth::Identity,
    error::{Error, Result},
    model::{Record, Room},
    storage::{Archive, Store},
};
use actor::Request;
use settlers_protocol::*;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
#[derive(Clone)]
pub struct GameService {
    inner: Arc<Inner>,
}
struct Inner {
    capacity: usize,
    metrics: crate::metrics::Metrics,
    creation: tokio::sync::Mutex<()>,
    rooms: RwLock<BTreeMap<String, Handle>>,
    store: Arc<dyn Store>,
    archive: Arc<dyn Archive>,
    sent: tokio::sync::Mutex<std::collections::HashMap<(String, u64), tokio::time::Instant>>,
}
#[derive(Clone)]
struct Handle {
    queue: mpsc::Sender<Request>,
    events: broadcast::Sender<Arc<Record>>,
}
impl GameService {
    pub async fn restore(
        store: Arc<dyn Store>,
        archive: Arc<dyn Archive>,
        now: u64,
    ) -> Result<Self> {
        Self::restore_with_capacity(store, archive, now, usize::MAX).await
    }
    pub async fn restore_with_capacity(
        store: Arc<dyn Store>,
        archive: Arc<dyn Archive>,
        now: u64,
        capacity: usize,
    ) -> Result<Self> {
        let service = Self {
            inner: Arc::new(Inner {
                capacity,
                metrics: crate::metrics::Metrics::default(),
                creation: tokio::sync::Mutex::new(()),
                rooms: RwLock::new(BTreeMap::new()),
                store,
                archive,
                sent: tokio::sync::Mutex::new(Default::default()),
            }),
        };
        if let Err(error) = service.flush_archive().await {
            tracing::warn!(error=%error, "archive recovery deferred; restoring durable snapshots");
        }
        for id in service.inner.store.ids().await? {
            if let Some(mut room) = service.inner.store.load(&id).await? {
                if service.loaded_games().await >= capacity {
                    return Err(Error::new(
                        ErrorCode::Backpressure,
                        "assigned games exceed server capacity",
                    ));
                }
                if room.schema != 1 {
                    return Err(Error::new(
                        ErrorCode::Internal,
                        "unsupported persisted room schema",
                    ));
                }
                for subject in room
                    .members
                    .iter()
                    .filter(|m| m.connected)
                    .map(|m| m.identity.subject.clone())
                    .collect::<Vec<_>>()
                {
                    let (next, record) = crate::reducer::reduce(
                        &room,
                        crate::model::Input::Presence {
                            subject,
                            connected: false,
                        },
                        now,
                    )?;
                    if let Err(error) = service
                        .inner
                        .store
                        .commit(room.version, &next, &record)
                        .await
                    {
                        tracing::warn!(game_id=%id, error=%error, "presence recovery deferred to actor tick");
                        // A full outbox must not prevent read access to the last durable state.
                        // The actor reconciles socket presence once commits are available again.
                        room = service.inner.store.load(&id).await?.ok_or_else(closed)?;
                        break;
                    }
                    room = next;
                }
                service.insert(room).await;
            }
        }
        Ok(service)
    }
    async fn insert(&self, room: Room) {
        let (queue, receiver) = mpsc::channel(256);
        let (events, _) = broadcast::channel(256);
        let handle = Handle {
            queue,
            events: events.clone(),
        };
        self.inner
            .rooms
            .write()
            .await
            .insert(room.id.clone(), handle);
        tokio::spawn(actor::run(
            room,
            receiver,
            events,
            Arc::clone(&self.inner.store),
        ));
    }
    async fn handle(&self, id: &str) -> Result<Handle> {
        self.inner
            .rooms
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "game not found"))
    }
    /// Drop actors whose durable ownership was withdrawn before reusing their capacity.
    pub async fn sync_ownership(&self) -> Result<()> {
        let _creation = self.inner.creation.lock().await;
        let ids: std::collections::BTreeSet<_> =
            self.inner.store.ids().await?.into_iter().collect();
        self.inner
            .rooms
            .write()
            .await
            .retain(|id, _| ids.contains(id));
        self.inner
            .sent
            .lock()
            .await
            .retain(|(id, _), _| ids.contains(id));
        Ok(())
    }
    pub async fn loaded_games(&self) -> usize {
        self.inner.rooms.read().await.len()
    }
    pub fn observe_tick_lag(&self, milliseconds: u64) {
        self.inner.metrics.tick_lag(milliseconds);
    }
    pub fn take_metrics(&self) -> (u64, u64, u64) {
        self.inner.metrics.take()
    }
    pub async fn create(
        &self,
        identity: Identity,
        request: CreateGame,
        now: u64,
    ) -> Result<GameView> {
        self.create_assigned(&uuid::Uuid::new_v4().to_string(), identity, request, now)
            .await
    }
    /// Create a stable assigned game once, recovering the same host's ambiguous retries.
    pub async fn create_assigned(
        &self,
        id: &str,
        identity: Identity,
        request: CreateGame,
        now: u64,
    ) -> Result<GameView> {
        let _creation = self.inner.creation.lock().await;
        if self.inner.rooms.read().await.contains_key(id) {
            let view = self.view(id, &identity.subject).await?;
            if !view.host {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "only the original host can recover creation",
                ));
            }
            return Ok(view);
        }
        if self.loaded_games().await >= self.inner.capacity {
            return Err(Error::new(
                ErrorCode::Backpressure,
                "server is waiting for released game capacity",
            ));
        }
        if let Some(room) = self.inner.store.load(id).await? {
            let view = crate::reducer::view(&room, &identity.subject)?;
            if !view.host {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "only the original host can recover creation",
                ));
            }
            self.insert(room).await;
            return Ok(view);
        }
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).map_err(Error::storage)?;
        let (room, record) =
            crate::reducer::create(id.to_owned(), request, identity.clone(), seed, now)?;
        self.inner.store.commit(0, &room, &record).await?;
        let view = crate::reducer::view(&room, &identity.subject)?;
        self.insert(room).await;
        Ok(view)
    }
    pub async fn execute(
        &self,
        id: &str,
        identity: Identity,
        request: CommandRequest,
        now: u64,
    ) -> Result<Receipt> {
        let started = std::time::Instant::now();
        let handle = self.handle(id).await?;
        let (reply, receive) = oneshot::channel();
        enqueue(
            &handle,
            Request::Execute {
                identity,
                request,
                now,
                reply,
            },
        )?;
        let result = receive.await.map_err(|_| closed())?;
        self.inner
            .metrics
            .command(started.elapsed().as_millis() as u64);
        result
    }
    /// Commit admission and seat removal atomically with the replay outbox.
    pub async fn admission(
        &self,
        id: &str,
        identity: Identity,
        request: AdmissionUpdate,
        now: u64,
    ) -> Result<()> {
        let handle = self.handle(id).await?;
        let (reply, receive) = oneshot::channel();
        enqueue(
            &handle,
            Request::Admission {
                identity,
                request,
                now,
                reply,
            },
        )?;
        receive.await.map_err(|_| closed())?
    }
    pub async fn room(&self, id: &str) -> Result<Room> {
        let handle = self.handle(id).await?;
        let (reply, receive) = oneshot::channel();
        enqueue(&handle, Request::Read { reply })?;
        receive.await.map_err(|_| closed())
    }
    pub async fn view(&self, id: &str, subject: &str) -> Result<GameView> {
        crate::reducer::view(&self.room(id).await?, subject)
    }
    pub async fn list(&self, after: Option<&str>, limit: usize) -> Result<GameList> {
        let ids: Vec<_> = self
            .inner
            .rooms
            .read()
            .await
            .keys()
            .filter(|id| after.is_none_or(|a| id.as_str() > a))
            .take(limit.clamp(1, 100) + 1)
            .cloned()
            .collect();
        let more = ids.len() > limit.clamp(1, 100);
        let mut games = vec![];
        for id in ids.iter().take(limit.clamp(1, 100)) {
            games.push(self.room(id).await?.summary());
        }
        Ok(GameList {
            next_after: if more {
                games.last().map(|g| g.id.clone())
            } else {
                None
            },
            games,
        })
    }
    pub async fn subscribe(
        &self,
        id: &str,
        subject: &str,
    ) -> Result<broadcast::Receiver<Arc<Record>>> {
        let handle = self.handle(id).await?;
        let receiver = handle.events.subscribe();
        if self.room(id).await?.member(subject).is_none() {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "join before opening a game socket",
            ));
        }
        Ok(receiver)
    }
    pub async fn presence(
        &self,
        id: &str,
        subject: String,
        session: String,
        connected: bool,
        now: u64,
    ) -> Result<()> {
        let handle = self.handle(id).await?;
        let (reply, receive) = oneshot::channel();
        enqueue(
            &handle,
            Request::Presence {
                subject,
                session,
                connected,
                now,
                reply,
            },
        )?;
        receive.await.map_err(|_| closed())?
    }
    pub async fn tick(&self, now: u64) -> Result<()> {
        use futures_util::{StreamExt, stream};
        let handles: Vec<_> = self.inner.rooms.read().await.values().cloned().collect();
        let mut requests = stream::iter(handles)
            .map(|handle| async move {
                let (reply, receive) = oneshot::channel();
                enqueue(&handle, Request::Tick { now, reply })?;
                receive.await.map_err(|_| closed())?
            })
            .buffer_unordered(32);
        let mut failure = None;
        while let Some(result) = requests.next().await {
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
fn enqueue(handle: &Handle, request: Request) -> Result<()> {
    handle.queue.try_send(request).map_err(|_| {
        Error::new(
            ErrorCode::Backpressure,
            "game command queue is full or unavailable",
        )
    })
}
fn closed() -> Error {
    Error::new(ErrorCode::Internal, "game actor is unavailable")
}
