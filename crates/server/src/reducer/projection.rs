use crate::{
    error::{Error, Result},
    model::*,
};
use settlers_protocol::*;

fn research_spectator(room: &Room, subject: &str) -> bool {
    // Revoking control of a seat does not grant spectator access to hidden hands.
    room.research
        && room
            .member(subject)
            .is_some_and(|member| member.seat.is_none())
}

fn display_name(room: &Room, subject: &str, fallback: &str) -> String {
    room.display_names.get(subject).map_or_else(
        || crate::identity::legacy_display_name(subject, fallback),
        |name| name.name.clone(),
    )
}
/// Redaction is shared by REST, reconnect snapshots, live events, and history.
pub fn view(room: &Room, subject: &str) -> Result<GameView> {
    if room.game.is_some() && room.member(subject).is_none() {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "join as a player or spectator first",
        ));
    }
    let seat = room.seat(subject).filter(|_| {
        room.member(subject)
            .is_some_and(|m| m.resolution.is_none() && m.computer_policy.is_none())
    });
    Ok(GameView {
        id: room.id.clone(),
        name: room.name.clone(),
        research: room.research,
        research_hands: research_spectator(room, subject)
            .then(|| {
                room.game.as_ref().map(|game| {
                    (0..game.settings.players)
                        .map(|seat| ResearchHand {
                            seat,
                            hand: game.players[seat as usize],
                        })
                        .collect()
                })
            })
            .flatten(),
        version: room.version,
        settings: room.settings,
        opening: room.opening.clone(),
        host: room.host == subject,
        members: room
            .members
            .iter()
            .map(|m| MemberView {
                seat: m.seat,
                idle: m.idle,
                disconnected_at_ms: m.disconnected_at_ms,
                resolution: m.resolution,
                computer_policy: m.computer_policy,
                name: display_name(room, &m.identity.subject, &m.identity.name),
                connected: m.connected || m.policy().is_some(),
                ready: m.ready || m.resolution.is_some() || m.computer_policy.is_some(),
                is_you: m.identity.subject == subject,
            })
            .collect(),
        game: room
            .game
            .as_ref()
            .map(|s| settlers_engine::observe(s, seat)),
        offers: room
            .offers
            .iter()
            .filter(|o| o.public.status == OfferStatus::Open)
            .filter(|o| seat.is_none_or(|p| o.rejected & (1 << p) == 0))
            .filter_map(|o| redact_offer(room, seat, &o.public))
            .collect(),
        filters: seat.map(|p| room.filters[p as usize]),
        deadline_ms: room.deadline_ms,
        discard: seat.and_then(|p| {
            room.discard.as_ref().map(|d| DiscardView {
                id: d.id,
                required: d.required[p as usize],
                locked: d.locked[p as usize],
                draft: d.drafts[p as usize],
                deadline_ms: d.deadline_ms,
            })
        }),
    })
}
fn redact_offer(room: &Room, seat: Option<u8>, offer: &TradeOffer) -> Option<TradeOffer> {
    let mut offer = offer.clone();
    if let Some(p) = seat
        && p != offer.from
    {
        let filter = room.filters[p as usize][offer.from as usize];
        if filter.auto_reject_trades && offer.to.contains(&p) {
            return None;
        }
        if filter.mute_trade_messages || filter.shadow_mute {
            offer.message = None;
        }
    }
    Some(offer)
}
pub fn batch(room: &Room, subject: &str, record: &Record) -> Result<EventBatch> {
    if room.member(subject).is_none() {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "join before reading event history",
        ));
    }
    let seat = room.seat(subject).filter(|_| {
        room.member(subject)
            .is_some_and(|m| m.resolution.is_none() && m.computer_policy.is_none())
    });
    let events = record
        .public
        .iter()
        .filter_map(|event| match event {
            PublicEvent::Joined { seat, name } | PublicEvent::Presence { seat, name, .. } => {
                let subject = match &record.input {
                    Input::Create { identity, .. }
                    | Input::Command { identity, .. }
                    | Input::DisplayName { identity, .. } => Some(identity.subject.as_str()),
                    Input::Presence { subject, .. } => Some(subject.as_str()),
                    _ => None,
                };
                let name = subject.map_or_else(
                    || crate::identity::legacy_display_name("google:legacy", name),
                    |subject| display_name(room, subject, name),
                );
                Some(match event {
                    PublicEvent::Presence { connected, .. } => PublicEvent::Presence {
                        seat: *seat,
                        name,
                        connected: *connected,
                    },
                    _ => PublicEvent::Joined { seat: *seat, name },
                })
            }
            PublicEvent::TheftDetails { player, victim, .. } => (seat
                .is_some_and(|p| p == *player || p == *victim)
                || research_spectator(room, subject))
            .then(|| event.clone()),
            PublicEvent::Message { from, to, .. } => {
                if let Some(p) = seat {
                    if p != *from
                        && (to.as_ref().is_some_and(|r| !r.contains(&p))
                            || room.filters[p as usize][*from as usize].mute_chat
                            || room.filters[p as usize][*from as usize].shadow_mute)
                    {
                        None
                    } else {
                        Some(event.clone())
                    }
                } else if to.is_none() || research_spectator(room, subject) {
                    Some(event.clone())
                } else {
                    None
                }
            }
            PublicEvent::TradeOffered { offer } => {
                redact_offer(room, seat, offer).map(|offer| PublicEvent::TradeOffered { offer })
            }
            _ => Some(event.clone()),
        })
        .collect();
    Ok(EventBatch {
        sequence: record.sequence,
        timestamp_ms: record.timestamp_ms,
        events,
    })
}
