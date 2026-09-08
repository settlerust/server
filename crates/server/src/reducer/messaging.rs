use crate::{
    error::{Error, Result},
    model::Room,
};
use settlers_protocol::*;
pub(super) fn validate(message: Option<&Communication>) -> Result<()> {
    if message.is_some_and(|m| {
        m.text.as_ref().is_some_and(|s| {
            s.len() > 1024 || s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
        })
    }) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "message text exceeds 1024 bytes or contains control characters",
        ));
    }
    Ok(())
}
pub(super) fn rate_limit(room: &mut Room, p: u8, now: u64) -> Result<()> {
    if now.saturating_sub(room.chat_window_ms[p as usize]) >= 60000 {
        room.chat_window_ms[p as usize] = now;
        room.chat_count[p as usize] = 0;
    }
    if room.chat_count[p as usize] >= 16 {
        return Err(Error::new(
            ErrorCode::Backpressure,
            "communication rate limit reached; retry next minute",
        ));
    }
    room.chat_count[p as usize] += 1;
    Ok(())
}
pub(super) fn communicate(
    room: &mut Room,
    p: u8,
    to: &Option<Vec<u8>>,
    message: &Communication,
    now: u64,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    validate(Some(message))?;
    rate_limit(room, p, now)?;
    if to.as_ref().is_some_and(|v| {
        v.is_empty() || v.len() > 3 || v.iter().any(|&t| t >= room.settings.players || t == p)
    }) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "invalid message recipients",
        ));
    }
    events.push(PublicEvent::Message {
        from: p,
        to: to.clone(),
        message: message.clone(),
    });
    Ok(())
}
