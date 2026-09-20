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
pub(super) async fn run(
    mut room: Room,
    mut queue: mpsc::Receiver<Request>,
    events: broadcast::Sender<Arc<Record>>,
    store: Arc<dyn Store>,
) {
    let mut sessions = HashMap::<String, HashMap<String, bool>>::new();
    let mut computer: Option<tokio::task::JoinHandle<super::expert::Decision>> = None;
    let mut offered = HashMap::new();
    loop {
        let request = tokio::select! {
            request = queue.recv() => { let Some(request) = request else { break; }; request },
            result = async { if let Some(task) = &mut computer { task.await } else { std::future::pending().await } }, if computer.is_some() => {
                computer = None;
                if let Ok(Ok((seat, request))) = result {
                    let proposal = matches!(request.command, Command::OfferTrade { .. });
                    if request.expected_version == room.version && commit(&mut room, &store, &events, Input::ComputerCommand { seat, request }, crate::config::now_ms()).await.is_ok() && proposal {
                        offered.insert(seat, room.game.map_or(0, |game| game.turn));
                    }
                }
                continue;
            }
        };
        match request {
            Request::Activity {
                subject,
                session,
                idle,
                now,
                reply,
            } => {
                let result = if let Some(active) = sessions.get_mut(&subject) {
                    if let Some(status) = active.get_mut(&session) {
                        *status = idle;
                        let idle = active.values().all(|idle| *idle);
                        if room
                            .member(&subject)
                            .is_some_and(|m| m.idle != idle && m.connected)
                        {
                            commit(
                                &mut room,
                                &store,
                                &events,
                                Input::Activity { subject, idle },
                                now,
                            )
                            .await
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    }
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Request::DisplayName {
                identity,
                revision,
                now,
                reply,
            } => {
                let result = if (room.member(&identity.subject).is_none()
                    && !room.display_names.contains_key(&identity.subject))
                    || room
                        .display_names
                        .get(&identity.subject)
                        .is_some_and(|name| name.revision >= revision)
                {
                    Ok(())
                } else {
                    commit(
                        &mut room,
                        &store,
                        &events,
                        Input::DisplayName { identity, revision },
                        now,
                    )
                    .await
                };
                let _ = reply.send(result);
            }
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
                if let Err(error) = expire(&mut room, &store, &events, now).await {
                    let _ = reply.send(Err(error));
                    continue;
                }
                let active = sessions.entry(subject.clone()).or_default();
                if connected {
                    active.insert(session, false);
                } else {
                    active.remove(&session);
                }
                let connected = !active.is_empty();
                let idle = connected && active.values().all(|idle| *idle);
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
                        Input::Presence {
                            subject: subject.clone(),
                            connected,
                        },
                        now,
                    )
                    .await
                };
                let result = if result.is_ok()
                    && connected
                    && room.member(&subject).is_some_and(|m| m.idle != idle)
                {
                    commit(
                        &mut room,
                        &store,
                        &events,
                        Input::Activity { subject, idle },
                        now,
                    )
                    .await
                } else {
                    result
                };
                let _ = reply.send(result);
            }
            Request::Tick { now, reply } => {
                let mut result = Ok(());
                for subject in room
                    .members
                    .iter()
                    .filter(|m| {
                        (m.seat.is_some()
                            && !m.connected
                            && m.disconnected_at_ms.is_none()
                            && m.resolution.is_none())
                            || m.connected
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
                if result.is_ok()
                    && computer.is_none()
                    && let Some((seat, view)) = super::expert::next_view(&room)
                {
                    let offered_turn = offered.get(&seat).copied();
                    computer = Some(tokio::task::spawn_blocking(move || {
                        super::expert::choose(seat, view, offered_turn)
                    }));
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
    if room
        .game
        .is_none_or(|game| !matches!(game.phase, settlers_engine::Phase::Finished { .. }))
        && room.members.iter().any(|m| {
            !m.connected
                && m.resolution.is_none()
                && m.seat.is_some()
                && m.disconnected_at_ms
                    .is_some_and(|at| now >= at.saturating_add(150_000))
        })
    {
        commit(room, store, events, Input::DisconnectTimeout, now).await?;
    }
    if room.next_deadline_ms().is_some_and(|d| d <= now) {
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
