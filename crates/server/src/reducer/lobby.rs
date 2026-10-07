use crate::{
    auth::{Identity, validate_name},
    error::{Error, Result},
    model::*,
};
use settlers_engine::{Phase, State};
use settlers_protocol::*;
pub(super) fn join(
    room: &mut Room,
    identity: &Identity,
    name: &str,
    spectator: bool,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    validate_name(name)?;
    if !spectator
        && room
            .admission
            .as_ref()
            .is_some_and(|a| !a.players.contains(&identity.subject))
    {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "the control plane has not admitted this player",
        ));
    }
    if room.member(&identity.subject).is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "identity already belongs to this room",
        ));
    }
    if room.members.len() >= 36 {
        return Err(Error::new(
            ErrorCode::Backpressure,
            "room participant limit reached",
        ));
    }
    let seat = if spectator {
        None
    } else {
        if room.game.is_some() {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "players must join before the game starts",
            ));
        }
        Some(
            (0..room.settings.players)
                .find(|&p| !room.members.iter().any(|m| m.seat == Some(p)))
                .ok_or_else(|| Error::new(ErrorCode::Conflict, "all player seats are occupied"))?,
        )
    };
    room.members.push(Member {
        identity: Identity {
            subject: identity.subject.clone(),
            name: name.into(),
        },
        seat,
        connected: false,
        ready: false,
        grace_turn: None,
        idle: false,
        disconnected_at_ms: None,
        resolution: None,
        computer_policy: None,
    });
    events.push(PublicEvent::Joined {
        seat,
        name: name.into(),
    });
    Ok(())
}
fn can_start(room: &Room, identity: &Identity, ready_decided: bool) -> Result<()> {
    if room.seat(&identity.subject).is_none() && identity.subject != room.host {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "only players or the host can start",
        ));
    }
    let players: Vec<_> = room.members.iter().filter(|m| m.seat.is_some()).collect();
    if players.len() != room.settings.players as usize {
        return Err(Error::new(
            ErrorCode::Conflict,
            "fill all player seats before starting",
        ));
    }
    if !ready_decided
        && players.iter().any(|m| {
            m.resolution.is_none() && m.computer_policy.is_none() && (!m.ready || !m.connected)
        })
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "every player must be connected and ready to play",
        ));
    }
    Ok(())
}
pub(super) fn set_ready(
    room: &mut Room,
    identity: &Identity,
    ready: bool,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    if room.game.is_some() {
        return Err(Error::new(ErrorCode::Conflict, "game already started"));
    }
    if room.opening.is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "readiness was confirmed before the opening rolls",
        ));
    }
    let member = room
        .members
        .iter_mut()
        .find(|m| m.identity.subject == identity.subject && m.seat.is_some())
        .ok_or_else(|| Error::new(ErrorCode::Forbidden, "only seated players can mark ready"))?;
    if ready && !member.connected {
        return Err(Error::new(
            ErrorCode::Conflict,
            "connect the game WebSocket before marking ready",
        ));
    }
    member.ready = ready;
    events.push(PublicEvent::ReadyChanged {
        seat: member.seat.unwrap_or_default(),
        ready,
    });
    Ok(())
}
pub(super) fn start(
    room: &mut Room,
    identity: &Identity,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    can_start(room, identity, room.opening.is_some())?;
    if room.game.is_some() {
        return Err(Error::new(ErrorCode::Conflict, "game already started"));
    }
    if room.members.iter().filter(|m| m.seat.is_some()).count() != room.settings.players as usize {
        return Err(Error::new(
            ErrorCode::Conflict,
            "fill all player seats before starting",
        ));
    }
    room.game = Some(if room.opening.is_some() {
        opening_state(room)?.0
    } else {
        // Without opening rolls the seed determines the first seat.
        State::new(room.settings, room.seed)?
    });
    events.push(PublicEvent::Started);
    for seat in room
        .members
        .iter()
        .filter(|m| m.resolution == Some(DisconnectResolution::Retire))
        .filter_map(|m| m.seat)
    {
        if let Some(game) = &room.game
            && !matches!(game.phase, Phase::Finished { .. })
        {
            let (next, effects) = settlers_engine::retire(game, seat)?;
            room.game = Some(next);
            events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
        }
    }
    Ok(())
}
fn opening_state(room: &Room) -> Result<(State, Opening)> {
    let mut game = State::new(room.settings, room.seed)?;
    let mut contenders: Vec<u8> = (0..room.settings.players).collect();
    let mut rolls = vec![];
    let mut round = 0;
    while contenders.len() > 1 {
        let mut highest = 0;
        let mut leaders = vec![];
        for &player in &contenders {
            let first = game.random.below(6) as u8 + 1;
            let second = game.random.below(6) as u8 + 1;
            rolls.push(OpeningRoll {
                player,
                first,
                second,
                round,
            });
            let total = first + second;
            if total > highest {
                highest = total;
                leaders.clear();
            }
            if total == highest {
                leaders.push(player);
            }
        }
        contenders = leaders;
        round += 1;
    }
    let first = contenders[0];
    game.first = first;
    game.current = first;
    Ok((
        game,
        Opening {
            rolls,
            first,
            order: (0..room.settings.players)
                .map(|p| (first + p) % room.settings.players)
                .collect(),
        },
    ))
}
pub(super) fn roll_for_order(
    room: &mut Room,
    identity: &Identity,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    can_start(room, identity, false)?;
    if room.game.is_some() || room.opening.is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "opening order is already decided",
        ));
    }
    if room.members.iter().filter(|m| m.seat.is_some()).count() != room.settings.players as usize {
        return Err(Error::new(
            ErrorCode::Conflict,
            "fill all player seats before rolling",
        ));
    }
    let (_, opening) = opening_state(room)?;
    events.push(PublicEvent::OrderDecided {
        opening: opening.clone(),
    });
    room.opening = Some(opening);
    Ok(())
}
pub(super) fn presence(
    room: &mut Room,
    subject: &str,
    connected: bool,
    now: u64,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let member = room
        .members
        .iter_mut()
        .find(|m| m.identity.subject == subject)
        .ok_or_else(|| Error::new(ErrorCode::Forbidden, "join before connecting"))?;
    let unchanged = member.connected == connected;
    let needs_clock = !connected && member.disconnected_at_ms.is_none();
    if unchanged && !needs_clock {
        return Err(Error::new(ErrorCode::Conflict, "presence has not changed"));
    }
    member.connected = connected;
    if !connected && room.game.is_none() && room.opening.is_none() && member.ready {
        member.ready = false;
        if let Some(seat) = member.seat {
            events.push(PublicEvent::ReadyChanged { seat, ready: false });
        }
    }
    if !connected && let (Some(game), Some(seat)) = (&room.game, member.seat) {
        let must_act = seat == game.current
            || matches!(game.phase,Phase::Discard{pending} if pending&(1<<seat)!=0);
        let grace_key = match game.phase {
            Phase::SetupSettlement { step } | Phase::SetupRoad { step, .. } => step as u32,
            _ => 8 + game.turn,
        };
        if must_act
            && room.settings.turn_seconds > 0
            && member.grace_turn != Some(grace_key)
            && !matches!(game.phase, Phase::Finished { .. })
        {
            member.grace_turn = Some(grace_key);
            room.deadline_ms = Some(
                room.deadline_ms
                    .unwrap_or(now)
                    .max(now + room.settings.turn_seconds as u64 * 1000),
            );
        }
    }
    events.push(PublicEvent::Presence {
        seat: member.seat,
        name: member.identity.name.clone(),
        connected,
    });
    Ok(())
}

pub(super) fn configure(
    room: &mut Room,
    identity: &Identity,
    settings: Settings,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    if identity.subject != room.host {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "only the host can configure a lobby",
        ));
    }
    if room.game.is_some() || room.opening.is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "settings are fixed once the game starts",
        ));
    }
    settings.validate()?;
    if room
        .members
        .iter()
        .any(|m| m.seat.is_some_and(|p| p >= settings.players))
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "new capacity would remove an occupied seat",
        ));
    }
    room.settings = settings;
    for member in &mut room.members {
        if member.ready {
            member.ready = false;
            if let Some(seat) = member.seat {
                events.push(PublicEvent::ReadyChanged { seat, ready: false });
            }
        }
    }
    events.push(PublicEvent::SettingsChanged { settings });
    Ok(())
}
