use crate::{
    auth::Identity,
    error::{Error, Result},
    model::{Input, Record, Room},
    reducer,
    storage::Store,
};
use settlers_protocol::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{broadcast, mpsc, oneshot};
pub(super) enum Request {
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
pub(super) async fn run(
    mut room: Room,
    mut queue: mpsc::Receiver<Request>,
    events: broadcast::Sender<Arc<Record>>,
    store: Arc<dyn Store>,
) {
    let mut sessions = HashMap::<String, HashSet<String>>::new();
    while let Some(request) = queue.recv().await {
        match request {
            Request::Admission {
                identity,
                request,
                now,
                reply,
            } => {
                let result = if room.host != identity.subject {
                    Err(Error::new(
                        ErrorCode::Forbidden,
                        "only the host can update admission",
                    ))
                } else if room.admission.as_ref() == Some(&request) {
                    Ok(())
                } else {
                    commit(
                        &mut room,
                        &store,
                        &events,
                        Input::Admission { identity, request },
                        now,
                    )
                    .await
                };
                sessions.retain(|subject, _| room.member(subject).is_some());
                let _ = reply.send(result);
            }
            Request::Read { reply } => {
                let _ = reply.send(room.clone());
            }
            Request::Execute {
                identity,
                request,
                now,
                reply,
            } => {
                let result = execute(&mut room, &store, &events, identity, request, now).await;
                let _ = reply.send(result);
            }
            Request::Presence {
                subject,
                session,
                connected,
                now,
                reply,
            } => {
                let active = sessions.entry(subject.clone()).or_default();
                if connected {
                    active.insert(session);
                } else {
                    active.remove(&session);
                }
                let connected = !active.is_empty();
                let result = if room
                    .member(&subject)
                    .is_some_and(|m| m.connected == connected)
                {
                    Ok(())
                } else {
                    commit(
                        &mut room,
                        &store,
                        &events,
                        Input::Presence { subject, connected },
                        now,
                    )
                    .await
                };
                let _ = reply.send(result);
            }
            Request::Tick { now, reply } => {
                let mut result = Ok(());
                for subject in room
                    .members
                    .iter()
                    .filter(|m| {
                        m.connected
                            != sessions
                                .get(&m.identity.subject)
                                .is_some_and(|s| !s.is_empty())
                    })
                    .map(|m| m.identity.subject.clone())
                    .collect::<Vec<_>>()
                {
                    let connected = sessions.get(&subject).is_some_and(|s| !s.is_empty());
                    if let Err(error) = commit(
                        &mut room,
                        &store,
                        &events,
                        Input::Presence { subject, connected },
                        now,
                    )
                    .await
                    {
                        result = Err(error);
                        break;
                    }
                }
                if result.is_ok() {
                    result = expire(&mut room, &store, &events, now).await;
                }
                let _ = reply.send(result);
            }
        }
    }
}
async fn execute(
    room: &mut Room,
    store: &Arc<dyn Store>,
    events: &broadcast::Sender<Arc<Record>>,
    identity: Identity,
    request: CommandRequest,
    now: u64,
) -> Result<Receipt> {
    if let Some(receipt) = reducer::duplicate(room, &identity, &request)? {
        return Ok(receipt);
    }
    expire(room, store, events, now).await?;
    let request_id = request.request_id.clone();
    commit(
        room,
        store,
        events,
        Input::Command { identity, request },
        now,
    )
    .await?;
    Ok(Receipt {
        request_id,
        version: room.version,
        duplicate: false,
    })
}
async fn expire(
    room: &mut Room,
    store: &Arc<dyn Store>,
    events: &broadcast::Sender<Arc<Record>>,
    now: u64,
) -> Result<()> {
    if room.deadline_ms.is_some_and(|d| d <= now) {
        commit(room, store, events, Input::Timeout, now).await?;
    }
    Ok(())
}
async fn commit(
    room: &mut Room,
    store: &Arc<dyn Store>,
    events: &broadcast::Sender<Arc<Record>>,
    input: Input,
    now: u64,
) -> Result<()> {
    let (next, record) = reducer::reduce(room, input, now)?;
    if let Err(error) = store.commit(room.version, &next, &record).await {
        // Reconcile ambiguous commit outcomes. Broadcast a durable state only.
        if let Some(actual) = store.load(&room.id).await? {
            if actual.version == next.version && reducer::state_hash(&actual)? == record.state_hash
            {
                *room = actual;
                let _ = events.send(Arc::new(record));
                return Ok(());
            }
            if actual.version != room.version {
                *room = actual;
                return Err(Error::conflict(room.version));
            }
        }
        return Err(error);
    }
    *room = next;
    let _ = events.send(Arc::new(record));
    Ok(())
}
