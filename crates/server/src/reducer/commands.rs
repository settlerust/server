use super::{base_v9::seat, discard, lobby, messaging, presence, trades};
use crate::{
    auth::Identity,
    error::{Error, Result},
    model::Room,
};
use settlers_protocol::*;
pub(super) fn command(
    room: &mut Room,
    identity: &Identity,
    command: &Command,
    id: &str,
    now: u64,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    if let Command::Join { name, spectator } = command {
        if room.research && !spectator {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "research tables only admit agent seats; join as a spectator or use join_agent",
            ));
        }
        return lobby::join(room, identity, name, *spectator, events);
    }
    if let Command::JoinAgent { name } = command {
        return lobby::join(room, identity, name, false, events);
    }
    if room.member(&identity.subject).is_none() {
        return Err(Error::new(ErrorCode::Forbidden, "join this game first"));
    }
    if room
        .member(&identity.subject)
        .is_some_and(|m| m.resolution.is_some() || m.computer_policy.is_some())
        && !matches!(command, Command::ResolveDisconnect { .. })
    {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "this seat is no longer controlled by its former player",
        ));
    }
    match command {
        Command::ResolveDisconnect {
            seat,
            disconnected_at_ms,
            resolution,
        } => {
            if room.host != identity.subject {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "only the host can resolve a disconnected seat",
                ));
            }
            presence::resolve(room, *seat, *disconnected_at_ms, *resolution, now, events)
        }
        Command::SetReady { ready } => lobby::set_ready(room, identity, *ready, events),
        Command::Start => lobby::start(room, identity, events),
        Command::RollForOrder => lobby::roll_for_order(room, identity, events),
        Command::Configure { settings } => lobby::configure(room, identity, *settings, events),
        Command::Act { action } => {
            if matches!(action, settlers_engine::Action::Trade { .. }) {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "player trades require an authenticated offer acceptance",
                ));
            }
            let p = seat(room, identity)?;
            let game = room
                .game
                .as_ref()
                .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "game has not started"))?;
            if let settlers_engine::Action::Discard { resources } = action {
                discard::validate_full(room, p, *resources)?;
            }
            let (game, effects) = settlers_engine::apply_recorded(game, p, *action)?;
            room.game = Some(game);
            events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
            Ok(())
        }
        Command::SelectDiscard {
            discard_id,
            resources,
        } => {
            let p = seat(room, identity)?;
            discard::select(room, p, *discard_id, *resources, now)
        }
        Command::ConfirmDiscard { discard_id } => {
            let p = seat(room, identity)?;
            discard::confirm(room, p, *discard_id, now, events)
        }
        Command::OfferTrade {
            to,
            give,
            receive,
            message,
            expires_in_ms,
        } => {
            let p = seat(room, identity)?;
            messaging::validate(message.as_ref())?;
            messaging::rate_limit(room, p, now)?;
            let lifetime = expires_in_ms.unwrap_or(30_000);
            if !(1_000..=300_000).contains(&lifetime) {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "offer lifetime must be between 1 and 300 seconds",
                ));
            }
            trades::offer(
                room,
                p,
                TradeOffer {
                    id: id.into(),
                    from: p,
                    to: to.clone(),
                    give: *give,
                    receive: *receive,
                    message: message.clone(),
                    status: OfferStatus::Open,
                    declined: Vec::new(),
                    thinking: Vec::new(),
                    created_at_ms: Some(now),
                    expires_at_ms: Some(
                        now.saturating_add(u64::from(lifetime))
                            .min(room.deadline_ms.unwrap_or(u64::MAX)),
                    ),
                },
                events,
            )
        }
        Command::AcceptTrade { offer_id } => {
            let p = seat(room, identity)?;
            trades::accept(room, p, offer_id, events)
        }
        Command::RejectTrade { offer_id } => {
            let p = seat(room, identity)?;
            trades::reject(room, p, offer_id, events)
        }
        Command::ThinkTrade { offer_id } => {
            let p = seat(room, identity)?;
            trades::think(room, p, offer_id, events)
        }
        Command::CancelTrade { offer_id } => {
            let p = seat(room, identity)?;
            trades::cancel(room, p, offer_id, events)
        }
        Command::Communicate { to, message } => {
            let p = seat(room, identity)?;
            messaging::communicate(room, p, to, message, now, events)
        }
        Command::SetFilter { player, filter } => {
            let p = seat(room, identity)?;
            if *player >= room.settings.players || *player == p {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "filter must target another seat",
                ));
            }
            room.filters[p as usize][*player as usize] = *filter;
            Ok(())
        }
        Command::Join { .. } | Command::JoinAgent { .. } => Ok(()),
    }
}
