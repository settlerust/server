use crate::{
    auth::Identity,
    error::{Error, Result},
    model::{Input, Member, Record, ReplayVersion, Room},
    reducer::replay::record,
};
use settlers_protocol::{CreateGame, ErrorCode, Filter, PublicEvent};
/// Construct a private room and its first replay record from explicit identity, seed, and time.
pub fn create(
    id: String,
    request: CreateGame,
    identity: Identity,
    seed: [u8; 32],
    now: u64,
) -> Result<(Room, Record)> {
    create_version(id, request, identity, seed, now, ReplayVersion::CURRENT)
}
pub(super) fn create_version(
    id: String,
    request: CreateGame,
    identity: Identity,
    seed: [u8; 32],
    now: u64,
    version: ReplayVersion,
) -> Result<(Room, Record)> {
    match version {
        ReplayVersion::BaseV9 => create_base_v9(id, request, identity, seed, now),
    }
}
fn create_base_v9(
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
            idle: false,
            disconnected_at_ms: None,
            resolution: None,
            computer_policy: None,
        }],
        display_names: Default::default(),
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
        ReplayVersion::BaseV9,
    )?;
    Ok((room, record))
}
