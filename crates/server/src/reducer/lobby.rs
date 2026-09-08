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
        grace_turn: None,
    });
    events.push(PublicEvent::Joined {
        seat,
        name: name.into(),
    });
    Ok(())
}
pub(super) fn start(
    room: &mut Room,
    identity: &Identity,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    if identity.subject != room.host {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "only the host can start this lobby",
        ));
    }
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
        // Preserve the original transition for old clients and archived replays.
        State::new(room.settings, room.seed)?
    });
    events.push(PublicEvent::Started);
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
    if identity.subject != room.host {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "only the host can roll for order",
        ));
    }
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
    if member.connected == connected {
        return Err(Error::new(ErrorCode::Conflict, "presence has not changed"));
    }
    member.connected = connected;
    if !connected && let (Some(game), Some(seat)) = (&room.game, member.seat) {
        let must_act = seat == game.current
            || matches!(game.phase,Phase::Discard{pending} if pending&(1<<seat)!=0);
        let grace_key = match game.phase {
            Phase::SetupSettlement { step } | Phase::SetupRoad { step, .. } => step as u32,
            _ => 8 + game.turn,
        };
        if must_act
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
    events.push(PublicEvent::SettingsChanged { settings });
    Ok(())
}
