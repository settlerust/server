use crate::{
    auth::Identity,
    error::{Error, Result},
    model::Room,
};
use settlers_protocol::{AdmissionUpdate, ErrorCode, PublicEvent};

pub(super) fn update(
    room: &mut Room,
    identity: &Identity,
    request: &AdmissionUpdate,
    events: &mut Vec<PublicEvent>,
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
    let mut subjects = std::collections::HashSet::new();
    for computer in &request.computers {
        crate::auth::validate_name(&computer.name)?;
        if !request.players.contains(&computer.subject)
            || computer.subject == room.host
            || !subjects.insert(&computer.subject)
            || room
                .member(&computer.subject)
                .is_some_and(|m| m.computer_policy.is_none())
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid built-in computer seat",
            ));
        }
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
    // A former built-in identity cannot become a remotely controlled seat.
    room.members
        .retain(|m| m.computer_policy.is_none() || subjects.contains(&m.identity.subject));
    room.admission = Some(request.clone());
    for computer in &request.computers {
        if room.member(&computer.subject).is_none() {
            let identity = Identity {
                subject: computer.subject.clone(),
                name: computer.name.clone(),
            };
            super::lobby::join(room, &identity, &computer.name, false, events)?;
            room.opening = None;
        }
        if let Some(member) = room
            .members
            .iter_mut()
            .find(|m| m.identity.subject == computer.subject)
        {
            member.computer_policy = Some(computer.policy);
            member.ready = true;
        }
    }
    Ok(())
}
