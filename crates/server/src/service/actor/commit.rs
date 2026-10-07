//! Only durable adoption advances the actor or publishes a replay record.
use super::*;
pub(super) async fn execute(
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
pub(super) async fn expire(
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
                && m.computer_policy.is_none()
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
pub(super) async fn commit(
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
            actual.validate()?;
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
