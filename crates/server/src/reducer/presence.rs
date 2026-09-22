use crate::{
    error::{Error, Result},
    model::Room,
};
use settlers_protocol::*;

pub(super) fn connection_clock(room: &mut Room, subject: &str, connected: bool, now: u64) {
    if let Some(m) = room
        .members
        .iter_mut()
        .find(|m| m.identity.subject == subject)
    {
        m.idle = false;
        if m.resolution.is_none() {
            m.disconnected_at_ms = if connected { None } else { Some(now) };
        }
    }
}
pub(super) fn activity(
    room: &mut Room,
    subject: &str,
    idle: bool,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let member = room
        .members
        .iter_mut()
        .find(|m| m.identity.subject == subject && m.connected)
        .ok_or_else(|| Error::new(ErrorCode::Forbidden, "connect before reporting activity"))?;
    member.idle = idle;
    events.push(PublicEvent::Activity {
        seat: member.seat,
        idle,
    });
    Ok(())
}
pub(super) fn resolve(
    room: &mut Room,
    seat: u8,
    disconnected_at_ms: u64,
    resolution: DisconnectResolution,
    now: u64,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    if room
        .game
        .is_some_and(|game| matches!(game.phase, Phase::Finished { .. }))
    {
        return Err(Error::new(ErrorCode::Conflict, "the game has finished"));
    }
    let member = room
        .members
        .iter_mut()
        .find(|m| m.seat == Some(seat))
        .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "unknown seat"))?;
    if member.connected
        || member.resolution.is_some()
        || member.disconnected_at_ms != Some(disconnected_at_ms)
        || now < disconnected_at_ms.saturating_add(120_000)
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "the player reconnected or the recovery window is still open",
        ));
    }
    // Once the decision deadline has elapsed, only the automatic Expert outcome is valid.
    if now >= disconnected_at_ms.saturating_add(150_000)
        && resolution != DisconnectResolution::Expert
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "the decision deadline has passed",
        ));
    }
    member.resolution = Some(resolution);
    member.idle = false;
    events.push(PublicEvent::DisconnectResolved { seat, resolution });
    if resolution == DisconnectResolution::Retire
        && let Some(game) = room.game.as_ref()
    {
        let (game, effects) = settlers_engine::retire(game, seat)?;
        room.game = Some(game);
        events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
    }
    if let Some(discard) = &mut room.discard {
        if resolution == DisconnectResolution::Retire {
            discard.required[seat as usize] = 0;
        }
        // Partial selections have not moved any cards. A new controller must be
        // able to make its own full discard without inheriting an invisible lock.
        discard.locked[seat as usize] = Resources::default();
        discard.drafts[seat as usize] = Resources::default();
    }
    // A takeover must not inherit the former player's private mute/trade preferences.
    room.filters[seat as usize] = [Filter::default(); 4];
    Ok(())
}
pub(super) fn expire(room: &mut Room, now: u64, events: &mut Vec<PublicEvent>) -> Result<()> {
    let expired: Vec<_> = room
        .members
        .iter()
        .filter(|m| !m.connected && m.resolution.is_none() && m.computer_policy.is_none())
        .filter_map(|m| m.seat.zip(m.disconnected_at_ms))
        .filter(|(_, at)| now >= at.saturating_add(150_000))
        .collect();
    for (seat, at) in expired {
        resolve(room, seat, at, DisconnectResolution::Expert, now, events)?;
    }
    Ok(())
}
