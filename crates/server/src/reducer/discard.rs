use crate::{
    error::{Error, Result},
    model::{DiscardRound, Room},
};
use settlers_engine::{Action, Event, Phase, Resources};
use settlers_protocol::{ErrorCode, PublicEvent};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

pub(super) fn update(old: &Room, room: &mut Room, now: u64) {
    let pending = room.game.and_then(|g| match g.phase {
        Phase::Discard { pending } => Some(pending),
        _ => None,
    });
    if let Some(pending) = pending {
        if room.discard.is_none() {
            let Some(game) = room.game.as_ref() else {
                return;
            };
            let duration = (u64::from(room.settings.turn_seconds) * 1000 / 8).max(10_000);
            room.discard = Some(DiscardRound {
                id: room.version + 1,
                required: std::array::from_fn(|p| {
                    if pending & (1 << p) != 0 {
                        game.players[p].resources.total() / 2
                    } else {
                        0
                    }
                }),
                locked: [Resources::default(); 4],
                drafts: [Resources::default(); 4],
                deadline_ms: if room.settings.turn_seconds == 0 {
                    0
                } else {
                    now + duration
                },
                turn_remaining_ms: old.deadline_ms.unwrap_or(now).saturating_sub(now),
            });
        }
        // Presence, other seats' choices, and confirmations cannot extend this deadline.
        room.deadline_ms = room
            .discard
            .as_ref()
            .and_then(|d| (d.deadline_ms > 0).then_some(d.deadline_ms));
    } else if let Some(round) = room.discard.take() {
        room.deadline_ms =
            (room.settings.turn_seconds > 0).then_some(now + round.turn_remaining_ms);
    }
}
fn check(room: &Room, p: u8, id: u64, now: u64) -> Result<&DiscardRound> {
    let round = room
        .discard
        .as_ref()
        .ok_or_else(|| invalid("no discard round is active"))?;
    if id != round.id || (round.deadline_ms > 0 && now >= round.deadline_ms) {
        return Err(invalid("discard round has ended"));
    }
    if !room
        .game
        .is_some_and(|g| matches!(g.phase, Phase::Discard { pending } if pending & (1 << p) != 0))
    {
        return Err(invalid("this seat has no pending discard"));
    }
    Ok(round)
}
pub(super) fn select(
    room: &mut Room,
    p: u8,
    id: u64,
    resources: Resources,
    now: u64,
) -> Result<()> {
    let round = check(room, p, id, now)?;
    let locked = round.locked[p as usize];
    let game = room
        .game
        .as_ref()
        .ok_or_else(|| invalid("game has not started"))?;
    if resources.total() + locked.total() > round.required[p as usize]
        || (0..5).any(|r| {
            u16::from(resources.0[r]) + u16::from(locked.0[r])
                > u16::from(game.players[p as usize].resources.0[r])
        })
    {
        return Err(invalid(
            "selection exceeds your hand or remaining discard quota",
        ));
    }
    if let Some(round) = room.discard.as_mut() {
        round.drafts[p as usize] = resources;
    }
    Ok(())
}
pub(super) fn confirm(
    room: &mut Room,
    p: u8,
    id: u64,
    now: u64,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let round = check(room, p, id, now)?;
    if round.drafts[p as usize].total() == 0 {
        return Err(invalid("select cards before confirming"));
    }
    let mut complete = None;
    if let Some(round) = room.discard.as_mut() {
        for r in 0..5 {
            round.locked[p as usize].0[r] += round.drafts[p as usize].0[r];
        }
        round.drafts[p as usize] = Resources::default();
        if round.locked[p as usize].total() == round.required[p as usize] {
            complete = Some(round.locked[p as usize]);
        }
    }
    if let Some(resources) = complete {
        resolve(room, p, resources, events)?;
    }
    Ok(())
}
pub(super) fn validate_full(room: &Room, p: u8, resources: Resources) -> Result<()> {
    if room
        .discard
        .as_ref()
        .is_some_and(|d| !resources.contains(d.locked[p as usize]))
    {
        return Err(invalid("confirmed discard cards cannot be changed"));
    }
    Ok(())
}
fn resolve(
    room: &mut Room,
    p: u8,
    resources: Resources,
    events: &mut Vec<PublicEvent>,
) -> Result<()> {
    let game = room
        .game
        .as_ref()
        .ok_or_else(|| invalid("game has not started"))?;
    let (next, effects) = settlers_engine::apply_recorded(game, p, Action::Discard { resources })?;
    room.game = Some(next);
    events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
    Ok(())
}
pub(super) fn timeout(room: &mut Room, now: u64, events: &mut Vec<PublicEvent>) -> Result<()> {
    let game = room
        .game
        .as_ref()
        .ok_or_else(|| invalid("game has not started"))?;
    if matches!(game.phase, Phase::Discard { .. }) {
        let old = room.clone();
        update(&old, room, now);
        let round = room
            .discard
            .clone()
            .ok_or_else(|| invalid("no discard round"))?;
        for p in 0..room.settings.players {
            let game = room
                .game
                .as_ref()
                .ok_or_else(|| invalid("game has not started"))?;
            if !matches!(game.phase, Phase::Discard { pending } if pending & (1 << p) != 0) {
                continue;
            }
            let mut chosen = round.locked[p as usize];
            for r in 0..5 {
                chosen.0[r] += round.drafts[p as usize].0[r];
            }
            let mut hand = game.players[p as usize].resources;
            for r in 0..5 {
                hand.0[r] -= chosen.0[r];
            }
            for _ in chosen.total()..round.required[p as usize] {
                let r = (0..5).max_by_key(|&r| (hand.0[r], 5 - r)).unwrap_or(0);
                hand.0[r] -= 1;
                chosen.0[r] += 1;
            }
            events.push(PublicEvent::Rule {
                event: Event::TimedOut { player: p },
            });
            resolve(room, p, chosen, events)?;
        }
        // Only finish discards. The roller still chooses where to move the robber.
        return Ok(());
    }
    if game.phase == Phase::Roll {
        let (rolled, effects) = settlers_engine::apply_recorded(game, game.current, Action::Roll)?;
        if matches!(rolled.phase, Phase::Discard { .. }) {
            events.push(PublicEvent::Rule {
                event: Event::TimedOut {
                    player: game.current,
                },
            });
            events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
            room.game = Some(rolled);
            return Ok(());
        }
    }
    let (next, effects) = settlers_engine::timeout(game)?;
    room.game = Some(next);
    events.extend(effects.into_iter().map(|event| PublicEvent::Rule { event }));
    Ok(())
}
