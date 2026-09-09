use crate::{
    auth::Identity,
    error::{Error, Result},
    model::Room,
};
use settlers_protocol::{AdmissionUpdate, ErrorCode};

pub(super) fn update(
    room: &mut Room,
    identity: &Identity,
    request: &AdmissionUpdate,
) -> Result<()> {
    if room.host != identity.subject {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "only the host can update admission",
        ));
    }
    if request.players.len() > usize::from(room.settings.players)
        || request
            .players
            .iter()
            .any(|s| s.is_empty() || s.len() > 200)
        || request
            .players
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != request.players.len()
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "invalid admission list",
        ));
    }
    if room
        .admission
        .as_ref()
        .is_some_and(|old| request.revision <= old.revision)
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "admission revision must increase",
        ));
    }
    if room.game.is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "admission is fixed after the game starts",
        ));
    }
    if room.seat(&room.host).is_some() && !request.players.contains(&room.host) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "the host keeps their seat",
        ));
    }
    let previous = room.members.len();
    room.members
        .retain(|m| m.seat.is_none() || request.players.contains(&m.identity.subject));
    if previous != room.members.len() {
        room.opening = None;
    }
    room.admission = Some(request.clone());
    Ok(())
}
