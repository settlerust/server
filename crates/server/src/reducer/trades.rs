use crate::{
    error::{Error, Result},
    model::*,
};
use settlers_engine::{Action, Phase};
use settlers_protocol::*;
fn unavailable() -> Error {
    Error::new(
        ErrorCode::TradeUnavailable,
        "trade offer is no longer available",
    )
}
pub(super) fn offer(
    room: &mut Room,
    p: u8,
    offer: TradeOffer,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    if room.offers.iter().any(|o| o.public.id == offer.id) {
        return Err(Error::new(ErrorCode::Conflict, "offer ID already exists"));
    }
    let game = room.game.as_ref().ok_or_else(unavailable)?;
    if game.phase != Phase::Main
        || offer.to.is_empty()
        || offer.to.len() > 3
        || offer
            .to
            .iter()
            .any(|&t| t >= game.settings.players || t == p)
        || offer
            .to
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != offer.to.len()
        || (p != game.current && offer.to != vec![game.current])
        || offer.give.total() == 0
        || offer.receive.total() == 0
        || !offer.give.disjoint(offer.receive)
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "invalid trade offer for this turn",
        ));
    }
    if !game.players[p as usize].resources.contains(offer.give) {
        return Err(settlers_engine::RuleError::InsufficientResources.into());
    }
    let turn = game.turn;
    for previous in &mut room.offers {
        if !improves(&offer, &previous.public) {
            continue;
        }
        if previous.turn == Some(turn)
            && let OfferStatus::Accepted { player } = previous.public.status
        {
            let mut error = Error::new(
                ErrorCode::TradeTaken,
                "the earlier offer was accepted; its replacement was not published",
            );
            error.0.winner = Some(player);
            return Err(error);
        }
        if previous.public.status == OfferStatus::Open {
            close(previous, TradeCloseReason::Replaced, events);
        }
    }
    if room
        .offers
        .iter()
        .filter(|o| o.public.from == p && o.public.status == OfferStatus::Open)
        .count()
        >= 4
    {
        return Err(Error::new(
            ErrorCode::Backpressure,
            "cancel an open offer before creating another",
        ));
    }
    let mut rejected = 0;
    for &t in &offer.to {
        if room.filters[t as usize][p as usize].auto_reject_trades {
            rejected |= 1 << t;
        }
    }
    events.push(PublicEvent::TradeOffered {
        offer: offer.clone(),
    });
    room.offers.push(Offer {
        public: offer,
        rejected,
        turn: Some(turn),
    });
    if let Some(offer) = room.offers.last_mut() {
        close_if_declined(offer, events);
    }
    while room.offers.len() > 64 {
        if let Some(i) = room
            .offers
            .iter()
            .position(|o| o.public.status != OfferStatus::Open)
        {
            room.offers.remove(i);
        } else {
            break;
        }
    }
    Ok(())
}
/// A replacement must improve the recipient's exact bundle and reach every old recipient.
fn improves(new: &TradeOffer, old: &TradeOffer) -> bool {
    new.from == old.from
        && new.to.len() == old.to.len()
        && old.to.iter().all(|p| new.to.contains(p))
        && new.give.contains(old.give)
        && old.receive.contains(new.receive)
        && (new.give != old.give || new.receive != old.receive)
}
fn close(offer: &mut Offer, reason: TradeCloseReason, events: &mut Vec<PublicEvent>) {
    offer.public.status = OfferStatus::Cancelled;
    events.push(PublicEvent::TradeClosed {
        offer_id: offer.public.id.clone(),
        reason,
        winner: None,
    });
}
fn close_if_declined(offer: &mut Offer, events: &mut Vec<PublicEvent>) {
    if offer
        .public
        .to
        .iter()
        .all(|p| offer.rejected & (1 << p) != 0)
    {
        close(offer, TradeCloseReason::Declined, events);
    }
}
/// Revalidate promises in the same transaction as every resource-changing action.
pub(super) fn prune(room: &mut Room, events: &mut Vec<PublicEvent>) {
    let Some(game) = &room.game else {
        return;
    };
    for offer in &mut room.offers {
        if offer.public.status == OfferStatus::Open
            && !game.players[offer.public.from as usize]
                .resources
                .contains(offer.public.give)
        {
            close(offer, TradeCloseReason::ResourcesChanged, events);
        }
    }
}
pub(super) fn accept(
    room: &mut Room,
    p: u8,
    id: &str,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let i = room
        .offers
        .iter()
        .position(|o| o.public.id == id)
        .ok_or_else(unavailable)?;
    let offer = &room.offers[i];
    if !offer.public.to.contains(&p) {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "offer was not addressed to this seat",
        ));
    }
    if let OfferStatus::Accepted { player } = offer.public.status {
        let mut error = Error::new(
            ErrorCode::TradeTaken,
            "trade declined because another player accepted it first",
        );
        error.0.winner = Some(player);
        return Err(error);
    }
    if offer.public.status != OfferStatus::Open
        || offer.rejected & (1 << p) != 0
        || room.filters[p as usize][offer.public.from as usize].auto_reject_trades
    {
        return Err(unavailable());
    }
    let game = room.game.as_ref().ok_or_else(unavailable)?;
    let (game, effects) = settlers_engine::apply_recorded(
        game,
        offer.public.from,
        Action::Trade {
            other: p,
            give: offer.public.give,
            receive: offer.public.receive,
        },
    )?;
    room.game = Some(game);
    room.offers[i].public.status = OfferStatus::Accepted { player: p };
    events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
    events.push(PublicEvent::TradeClosed {
        offer_id: id.into(),
        reason: TradeCloseReason::Accepted,
        winner: Some(p),
    });
    Ok(())
}
pub(super) fn reject(
    room: &mut Room,
    p: u8,
    id: &str,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let offer = room
        .offers
        .iter_mut()
        .find(|o| o.public.id == id)
        .ok_or_else(unavailable)?;
    if !offer.public.to.contains(&p) || offer.public.status != OfferStatus::Open {
        return Err(unavailable());
    }
    offer.rejected |= 1 << p;
    offer.public.thinking.retain(|player| *player != p);
    events.push(PublicEvent::TradeRejected {
        offer_id: id.into(),
        player: p,
    });
    if !offer.public.declined.contains(&p) {
        offer.public.declined.push(p);
    }
    close_if_declined(offer, events);
    Ok(())
}
pub(super) fn cancel(
    room: &mut Room,
    p: u8,
    id: &str,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let offer = room
        .offers
        .iter_mut()
        .find(|o| o.public.id == id)
        .ok_or_else(unavailable)?;
    if offer.public.from != p || offer.public.status != OfferStatus::Open {
        return Err(unavailable());
    }
    offer.public.status = OfferStatus::Cancelled;
    events.push(PublicEvent::TradeClosed {
        offer_id: id.into(),
        reason: TradeCloseReason::Cancelled,
        winner: None,
    });
    Ok(())
}
pub(super) fn think(room: &mut Room, p: u8, id: &str, events: &mut Vec<PublicEvent>) -> Result<()> {
    let offer = room
        .offers
        .iter_mut()
        .find(|o| o.public.id == id)
        .ok_or_else(unavailable)?;
    if !offer.public.to.contains(&p)
        || offer.public.status != OfferStatus::Open
        || offer.rejected & (1 << p) != 0
        || room.filters[p as usize][offer.public.from as usize].auto_reject_trades
    {
        return Err(unavailable());
    }
    if !offer.public.thinking.contains(&p) {
        offer.public.thinking.push(p);
        events.push(PublicEvent::TradeThinking {
            offer_id: id.into(),
            player: p,
        });
    }
    Ok(())
}
/// Offer clocks expire independently of the turn and never inspect a recipient's hand.
pub(super) fn expire_due(room: &mut Room, now: u64, events: &mut Vec<PublicEvent>) {
    for offer in &mut room.offers {
        if offer.public.status == OfferStatus::Open
            && offer
                .public
                .expires_at_ms
                .is_some_and(|deadline| deadline <= now)
        {
            offer.public.status = OfferStatus::Expired;
            events.push(PublicEvent::TradeClosed {
                offer_id: offer.public.id.clone(),
                reason: TradeCloseReason::Expired,
                winner: None,
            });
        }
    }
}
pub(super) fn expire(room: &mut Room, events: &mut Vec<PublicEvent>) {
    for offer in &mut room.offers {
        if offer.public.status == OfferStatus::Open {
            offer.public.status = OfferStatus::Expired;
            events.push(PublicEvent::TradeClosed {
                offer_id: offer.public.id.clone(),
                reason: TradeCloseReason::TurnEnded,
                winner: None,
            });
        }
    }
}
