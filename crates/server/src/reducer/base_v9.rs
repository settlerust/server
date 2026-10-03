//! Base-v9 room semantics have a named boundary so future defaults can use another reducer.
use super::{admission, commands::command, discard, lobby, presence, replay::record, trades};
use crate::{
    auth::Identity,
    error::{Error, Result},
    model::*,
};
use settlers_engine::Phase;
use settlers_protocol::*;
/// Validate one input and return the proposed snapshot and matching replay record.
/// The caller must durably commit both before publishing; an error leaves `input` unchanged.
pub(super) fn reduce(input: &Room, event: Input, now: u64) -> Result<(Room, Record)> {
    let mut room = input.clone();
    let mut public = vec![];
    trades::expire_due(&mut room, now, &mut public);
    match &event {
        Input::Activity { subject, idle } => {
            presence::activity(&mut room, subject, *idle, &mut public)?
        }
        Input::DisconnectTimeout => presence::expire(&mut room, now, &mut public)?,
        Input::ComputerCommand { seat, request } => {
            let member = room
                .members
                .iter_mut()
                .find(|m| m.seat == Some(*seat) && m.policy().is_some())
                .ok_or_else(|| {
                    Error::new(ErrorCode::Conflict, "computer no longer controls this seat")
                })?;
            let identity = member.identity.clone();
            let resolution = member.resolution.take();
            let policy = member.computer_policy.take();
            if request.expected_version != room.version {
                return Err(Error::conflict(room.version));
            }
            command(
                &mut room,
                &identity,
                &request.command,
                &request.request_id,
                now,
                &mut public,
            )?;
            if let Some(member) = room.members.iter_mut().find(|m| m.seat == Some(*seat)) {
                member.resolution = resolution;
                member.computer_policy = policy;
            }
        }
        Input::DisplayName { identity, revision } => {
            crate::auth::validate_name(&identity.name)?;
            if room.member(&identity.subject).is_none()
                && !room.display_names.contains_key(&identity.subject)
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "join before updating a display name",
                ));
            }
            if room
                .display_names
                .get(&identity.subject)
                .is_some_and(|old| old.revision >= *revision)
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "display name revision is stale",
                ));
            }
            room.display_names.insert(
                identity.subject.clone(),
                DisplayName {
                    name: identity.name.clone(),
                    revision: *revision,
                },
            );
            if let Some(member) = room
                .members
                .iter_mut()
                .find(|member| member.identity.subject == identity.subject)
            {
                member.identity.name.clone_from(&identity.name);
                public.push(PublicEvent::Presence {
                    seat: member.seat,
                    name: identity.name.clone(),
                    connected: member.connected,
                });
            }
        }
        Input::Admission { identity, request } => {
            admission::update(&mut room, identity, request, &mut public)?;
            public.push(PublicEvent::AdmissionChanged);
        }
        Input::Command { identity, request } => {
            if uuid::Uuid::parse_str(&request.request_id).is_err() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "request_id must be a UUID",
                ));
            }
            if request.expected_version > room.version
                || (request.expected_version != room.version
                    && !matches!(
                        request.command,
                        Command::AcceptTrade { .. }
                            | Command::SelectDiscard { .. }
                            | Command::ConfirmDiscard { .. }
                    ))
            {
                return Err(Error::conflict(room.version));
            }
            command(
                &mut room,
                identity,
                &request.command,
                &request.request_id,
                now,
                &mut public,
            )?;
            room.receipts.push(StoredReceipt {
                subject: identity.subject.clone(),
                request: request.clone(),
                version: room.version + 1,
            });
            if room.receipts.len() > 64 {
                room.receipts.remove(0);
            }
        }
        Input::Presence { subject, connected } => {
            lobby::presence(&mut room, subject, *connected, now, &mut public)?;
            presence::connection_clock(&mut room, subject, *connected, now);
        }
        Input::Timeout => {
            if room.deadline_ms.is_none_or(|d| d > now) && public.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "turn deadline has not expired",
                ));
            }
            if room.deadline_ms.is_some_and(|d| d <= now) {
                discard::timeout(&mut room, now, &mut public)?;
            }
        }
        Input::Create { .. } => {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "lobby already exists",
            ));
        }
    }
    update_deadline(input, &mut room, now);
    discard::update(input, &mut room, now);
    if input
        .game
        .zip(room.game)
        .is_some_and(|(a, b)| a.turn != b.turn || matches!(b.phase, Phase::Finished { .. }))
    {
        trades::expire(&mut room, &mut public);
    }
    trades::prune(&mut room, &mut public);
    append_theft_details(input, &room, &mut public);
    room.version += 1;
    let record = record(&room, event, now, public, ReplayVersion::BaseV9)?;
    Ok((room, record))
}
fn append_theft_details(before: &Room, after: &Room, events: &mut Vec<PublicEvent>) {
    let Some((before, after)) = before.game.zip(after.game) else {
        return;
    };
    let details: Vec<_> = events
        .iter()
        .filter_map(|event| {
            let PublicEvent::Rule {
                event:
                    settlers_engine::Event::Stole {
                        player,
                        victim,
                        count: 1,
                    },
            } = event
            else {
                return None;
            };
            settlers_engine::Resource::ALL
                .into_iter()
                .find(|resource| {
                    let index = *resource as usize;
                    before.players[*victim as usize].resources.0[index]
                        > after.players[*victim as usize].resources.0[index]
                })
                .map(|resource| PublicEvent::TheftDetails {
                    player: *player,
                    victim: *victim,
                    resource,
                })
        })
        .collect();
    events.extend(details);
}
pub(super) fn seat(room: &Room, identity: &Identity) -> Result<u8> {
    room.seat(&identity.subject)
        .ok_or_else(|| Error::new(ErrorCode::Forbidden, "spectators cannot act as players"))
}
fn update_deadline(old: &Room, new: &mut Room, now: u64) {
    let Some(game) = new.game else {
        return;
    };
    if new.settings.turn_seconds == 0 || matches!(game.phase, Phase::Finished { .. }) {
        new.deadline_ms = None;
        return;
    }
    let reset = old.game.is_none_or(|previous| {
        previous.turn != game.turn
            || previous.current != game.current
            || matches!(
                (previous.phase, game.phase),
                (Phase::SetupRoad { .. }, Phase::SetupSettlement { .. })
            )
    });
    if reset {
        new.deadline_ms = Some(now + new.settings.turn_seconds as u64 * 1000);
    }
}
