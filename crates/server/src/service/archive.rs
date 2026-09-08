use super::GameService;
use crate::{
    error::{Error, Result},
    storage::merge_records,
};
use settlers_protocol::*;
use std::collections::BTreeMap;
impl GameService {
    /// Retry delivery safely, then prune only the contiguous range verified in archive read-back.
    pub async fn flush_archive(&self) -> Result<()> {
        let mut sent = self.inner.sent.lock().await;
        let mut failure = None;
        for id in self.inner.store.ids().await? {
            if let Err(error) = self.flush_game_archive(&id, &mut sent).await {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    async fn flush_game_archive(
        &self,
        id: &str,
        sent: &mut std::collections::HashMap<(String, u64), tokio::time::Instant>,
    ) -> Result<()> {
        let pending = self.inner.store.pending(id).await?;
        if pending.is_empty() {
            return Ok(());
        }
        let unsent: Vec<_> = pending
            .iter()
            .filter(|r| {
                sent.get(&(id.to_owned(), r.sequence))
                    .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(300))
            })
            .cloned()
            .collect();
        if !unsent.is_empty() {
            self.inner.archive.deliver(&unsent).await?;
            for record in &unsent {
                sent.insert(
                    (id.to_owned(), record.sequence),
                    tokio::time::Instant::now(),
                );
            }
        }
        let archived = self.inner.archive.read(id).await?;
        let found: BTreeMap<_, _> = archived.iter().map(|r| (r.sequence, r)).collect();
        let mut through = None;
        for record in &pending {
            if found
                .get(&record.sequence)
                .is_some_and(|archived| **archived == *record)
            {
                through = Some(record.sequence);
            } else {
                break;
            }
        }
        if let Some(through) = through {
            self.inner.store.prune(id, through).await?;
            sent.retain(|(game, sequence), _| game != id || *sequence > through);
        }
        Ok(())
    }
    pub async fn history(
        &self,
        id: &str,
        subject: &str,
        after: u64,
        limit: usize,
    ) -> Result<EventPage> {
        let room = self.room(id).await?;
        if room.member(subject).is_none() {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "join before reading history",
            ));
        }
        if after > room.version {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "history cursor is ahead of this game",
            ));
        }
        // Read outbox first: a concurrent prune can only remove records already archived.
        let pending = self.inner.store.pending(id).await?;
        let archived = self.inner.archive.read(id).await?;
        let records = merge_records(archived.into_iter().chain(pending))?;
        let mut expected = after + 1;
        let mut events = vec![];
        for record in records
            .iter()
            .filter(|r| r.sequence > after && r.sequence <= room.version)
            .take(limit.clamp(1, 100))
        {
            if record.sequence != expected {
                break;
            }
            events.push(crate::reducer::batch(&room, subject, record)?);
            expected += 1;
        }
        let next_after = expected - 1;
        let archive_pending = next_after < room.version && events.len() < limit.clamp(1, 100);
        Ok(EventPage {
            game_id: id.into(),
            events,
            next_after,
            current_version: room.version,
            archive_pending,
        })
    }
}
