//! Serialize each game's requests, reconcile durable writes, and publish committed records.
//!
//! `room` is the last confirmed snapshot. Reducers build a proposed successor;
//! only `commit` replaces it after storage confirms the state and replay outbox.
use crate::{
    auth::Identity,
    error::{Error, Result},
    model::{Input, Record, Room},
    reducer,
    storage::Store,
};
use settlers_protocol::*;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{broadcast, mpsc, oneshot};
pub(super) enum Request {
    Activity {
        subject: String,
        session: String,
        idle: bool,
        now: u64,
        reply: oneshot::Sender<Result<()>>,
    },
    DisplayName {
        identity: Identity,
        revision: u64,
        now: u64,
        reply: oneshot::Sender<Result<()>>,
    },
    Admission {
        identity: Identity,
        request: AdmissionUpdate,
        now: u64,
        reply: oneshot::Sender<Result<()>>,
    },
    Execute {
        identity: Identity,
        request: CommandRequest,
        now: u64,
        reply: oneshot::Sender<Result<Receipt>>,
    },
    Read {
        reply: oneshot::Sender<Room>,
    },
    Presence {
        subject: String,
        session: String,
        connected: bool,
        now: u64,
        reply: oneshot::Sender<Result<()>>,
    },
    Tick {
        now: u64,
        reply: oneshot::Sender<Result<()>>,
    },
}
mod commit;
mod computer;
mod requests;
use commit::{commit, execute, expire};
struct Actor {
    room: Room,
    store: Arc<dyn Store>,
    events: broadcast::Sender<Arc<Record>>,
    sessions: HashMap<String, HashMap<String, bool>>,
    computer: Option<tokio::task::JoinHandle<super::expert::Decision>>,
    offered: HashMap<u8, u32>,
}
pub(super) async fn run(
    room: Room,
    mut queue: mpsc::Receiver<Request>,
    events: broadcast::Sender<Arc<Record>>,
    store: Arc<dyn Store>,
) {
    let mut actor = Actor {
        room,
        store,
        events,
        sessions: HashMap::new(),
        computer: None,
        offered: HashMap::new(),
    };
    loop {
        let request = tokio::select! {
            request = queue.recv() => { let Some(request) = request else { break; }; request },
            result = actor.wait_for_computer(), if actor.computer.is_some() => {
                actor.finish_computer(result).await;
                continue;
            }
        };
        actor.handle(request).await;
    }
}
