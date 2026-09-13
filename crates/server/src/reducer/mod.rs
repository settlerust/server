//! Pure room transitions: identity, trades, communication, and rule-engine adaptation.
mod admission;
mod discard;
mod lobby;
mod messaging;
mod projection;
mod trades;
use crate::{
    auth::Identity,
    error::{Error, Result},
    model::*,
};
pub use projection::*;
use settlers_engine::Phase;
use settlers_protocol::*;
use sha2::{Digest, Sha256};

pub fn create(
    id: String,
    request: CreateGame,
    identity: Identity,
    seed: [u8; 32],
    now: u64,
) -> Result<(Room, Record)> {
    crate::auth::validate_name(&request.name)?;
    request.settings.validate()?;
    if request.research && request.host_as_player {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "research table hosts must spectate; use join_agent for agent seats",
        ));
    }
    if let Some(name) = &request.player_name {
        crate::auth::validate_name(name)?;
    }
    let seed = match request.seed {
        Some(index) if request.research && request.settings.deterministic => {
            // A research host may pin the tape; the bytes stay private room state.
            let mut bytes = [0u8; 32];
            bytes[..4].copy_from_slice(&index.to_le_bytes());
            bytes
        }
        Some(_) => {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "a seed is accepted only for an agent-only research table in deterministic mode",
            ));
        }
        None => seed,
    };
    let room = Room {
        schema: 1,
        id,
        name: request.name.clone(),
        research: request.research,
        host: identity.subject.clone(),
        version: 1,
        settings: request.settings,
        seed,
        members: vec![Member {
            identity: Identity {
                subject: identity.subject.clone(),
                name: request
                    .player_name
                    .clone()
                    .unwrap_or_else(|| identity.name.clone()),
            },
            seat: request.host_as_player.then_some(0),
            connected: false,
            ready: false,
            grace_turn: None,
        }],
        game: None,
        opening: None,
        admission: None,
        offers: vec![],
        filters: [[Filter::default(); 4]; 4],
        receipts: vec![],
        deadline_ms: None,
        discard: None,
        chat_window_ms: [0; 4],
        chat_count: [0; 4],
    };
    let record = record(
        &room,
        Input::Create {
            request,
            identity,
            seed,
        },
        now,
        vec![PublicEvent::LobbyCreated],
    )?;
    Ok((room, record))
}
/// Returns the existing receipt only when both identity and command content match.
pub fn duplicate(
    room: &Room,
    identity: &Identity,
    request: &CommandRequest,
) -> Result<Option<Receipt>> {
    if let Some(old) = room
        .receipts
        .iter()
        .find(|r| r.subject == identity.subject && r.request.request_id == request.request_id)
    {
        if old.request != *request {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "request_id was already used for a different request",
            ));
        }
        return Ok(Some(Receipt {
            request_id: request.request_id.clone(),
            version: old.version,
            duplicate: true,
        }));
    }
    Ok(None)
}
pub fn reduce(input: &Room, event: Input, now: u64) -> Result<(Room, Record)> {
    reduce_version(input, event, now, "base-v8")
}
fn reduce_version(
    input: &Room,
    event: Input,
    now: u64,
    engine_version: &str,
) -> Result<(Room, Record)> {
    let legacy = engine_version == "base-v1";
    let mut room = input.clone();
    let mut public = vec![];
    let timed_trades = engine_version == "base-v8";
    if timed_trades {
        trades::expire_due(&mut room, now, &mut public);
    }
    match &event {
        Input::Admission { identity, request } => {
            admission::update(&mut room, identity, request)?;
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
                engine_version,
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
        Input::Presence { subject, connected } => lobby::presence(
            &mut room,
            subject,
            *connected,
            now,
            &mut public,
            matches!(engine_version, "base-v7" | "base-v8"),
        )?,
        Input::Timeout => {
            if room.deadline_ms.is_none_or(|d| d > now) && public.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "turn deadline has not expired",
                ));
            }
            if room.deadline_ms.is_some_and(|d| d <= now) {
                discard::timeout(&mut room, now, &mut public, legacy)?;
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
    if !legacy {
        discard::update(input, &mut room, now);
    }
    if input
        .game
        .zip(room.game)
        .is_some_and(|(a, b)| a.turn != b.turn || matches!(b.phase, Phase::Finished { .. }))
    {
        trades::expire(&mut room, &mut public);
    }
    if matches!(
        engine_version,
        "base-v4" | "base-v5" | "base-v6" | "base-v7" | "base-v8"
    ) {
        trades::prune(&mut room, &mut public);
    }
    if matches!(
        engine_version,
        "base-v3" | "base-v4" | "base-v5" | "base-v6" | "base-v7" | "base-v8"
    ) {
        append_theft_details(input, &room, &mut public);
    }
    room.version += 1;
    let record = record(&room, event, now, public)?;
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
fn command(
    room: &mut Room,
    identity: &Identity,
    command: &Command,
    id: &str,
    now: u64,
    events: &mut Vec<PublicEvent>,
    engine_version: &str,
) -> Result<()> {
    let modern = matches!(
        engine_version,
        "base-v4" | "base-v5" | "base-v6" | "base-v7" | "base-v8"
    );
    let require_ready = matches!(
        engine_version,
        "base-v5" | "base-v6" | "base-v7" | "base-v8"
    );
    let close_declined = matches!(engine_version, "base-v6" | "base-v7" | "base-v8");
    let lock_opening = matches!(engine_version, "base-v7" | "base-v8");
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
    match command {
        Command::SetReady { ready } => {
            lobby::set_ready(room, identity, *ready, events, lock_opening)
        }
        Command::Start => lobby::start(room, identity, events, require_ready, lock_opening),
        Command::RollForOrder => lobby::roll_for_order(room, identity, events, require_ready),
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
            let timed = engine_version == "base-v8";
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
                    created_at_ms: timed.then_some(now),
                    expires_at_ms: timed.then(|| {
                        now.saturating_add(u64::from(lifetime))
                            .min(room.deadline_ms.unwrap_or(u64::MAX))
                    }),
                },
                events,
                modern,
                close_declined,
            )
        }
        Command::AcceptTrade { offer_id } => {
            let p = seat(room, identity)?;
            trades::accept(room, p, offer_id, events)
        }
        Command::RejectTrade { offer_id } => {
            let p = seat(room, identity)?;
            trades::reject(room, p, offer_id, events, close_declined)
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
fn seat(room: &Room, identity: &Identity) -> Result<u8> {
    room.seat(&identity.subject)
        .ok_or_else(|| Error::new(ErrorCode::Forbidden, "spectators cannot act as players"))
}
fn update_deadline(old: &Room, new: &mut Room, now: u64) {
    let Some(game) = new.game else {
        return;
    };
    if matches!(game.phase, Phase::Finished { .. }) {
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
pub fn state_hash(room: &Room) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(room).map_err(Error::storage)?)
    ))
}
fn record(room: &Room, input: Input, now: u64, public: Vec<PublicEvent>) -> Result<Record> {
    Ok(Record {
        schema: 1,
        engine_version: "base-v8".into(),
        game_id: room.id.clone(),
        sequence: room.version,
        timestamp_ms: now,
        input,
        public,
        state_hash: state_hash(room)?,
    })
}
/// Reconstruct every persisted field and verify the archive's per-transition hash.
pub fn replay(records: &[Record]) -> Result<Room> {
    let mut room = None;
    for record in records {
        if record.schema != 1
            || !matches!(
                record.engine_version.as_str(),
                "base-v1"
                    | "base-v2"
                    | "base-v3"
                    | "base-v4"
                    | "base-v5"
                    | "base-v6"
                    | "base-v7"
                    | "base-v8"
            )
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "unsupported replay version",
            ));
        }
        let (next, generated) = match (&room, &record.input) {
            (
                None,
                Input::Create {
                    request,
                    identity,
                    seed,
                },
            ) => create(
                record.game_id.clone(),
                request.clone(),
                identity.clone(),
                *seed,
                record.timestamp_ms,
            )?,
            (Some(previous), _) => reduce_version(
                previous,
                record.input.clone(),
                record.timestamp_ms,
                &record.engine_version,
            )?,
            _ => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "archive must start with lobby creation",
                ));
            }
        };
        if next.id != record.game_id
            || next.version != record.sequence
            || state_hash(&next)? != record.state_hash
            || generated.public != record.public
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "replay sequence or state hash mismatch",
            ));
        }
        room = Some(next);
    }
    room.ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "empty archive"))
}
