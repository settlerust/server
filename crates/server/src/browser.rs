//! Browser storage/transport adapter over the same authoritative room reducer.
//! Checkpoints contain secrets and must stay inside the local server worker.
use crate::{
    error::{Error, Result},
    identity::Identity,
    model::{Input, Record, Room},
    reducer,
};
use serde::{Deserialize, Serialize};
use settlers_protocol::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(Default, Serialize, Deserialize)]
pub struct BrowserRoom {
    room: Option<Room>,
    records: Vec<Record>,
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(Error::storage)
}
fn decode<T: serde::de::DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json)
        .map_err(|_| Error::new(ErrorCode::InvalidRequest, "invalid local request"))
}
fn wire<T>(result: Result<T>) -> std::result::Result<T, String> {
    result.map_err(|e| serde_json::to_string(&e.0).unwrap_or_else(|_| "local server error".into()))
}
impl BrowserRoom {
    fn current(&self) -> Result<&Room> {
        self.room
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "local game not found"))
    }
    fn commit(&mut self, input: Input, now: u64) -> Result<()> {
        let (room, record) = reducer::reduce(self.current()?, input, now)?;
        self.room = Some(room);
        self.records.push(record);
        Ok(())
    }
}
#[wasm_bindgen]
impl BrowserRoom {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(
        &mut self,
        id: String,
        request: &str,
        identity: &str,
        seed: &[u8],
        now: f64,
    ) -> std::result::Result<String, String> {
        wire((|| {
            if self.room.is_some() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "local game already exists",
                ));
            }
            let seed = seed
                .try_into()
                .map_err(|_| Error::new(ErrorCode::InvalidRequest, "expected 32 seed bytes"))?;
            let identity: Identity = decode(identity)?;
            let (room, record) =
                reducer::create(id, decode(request)?, identity.clone(), seed, now as u64)?;
            let view = encode(&reducer::view(&room, &identity.subject)?)?;
            self.room = Some(room);
            self.records.push(record);
            Ok(view)
        })())
    }
    pub fn command(
        &mut self,
        identity: &str,
        request: &str,
        now: f64,
    ) -> std::result::Result<String, String> {
        wire((|| {
            let identity: Identity = decode(identity)?;
            let request: CommandRequest = decode(request)?;
            if let Some(receipt) = reducer::duplicate(self.current()?, &identity, &request)? {
                return encode(&receipt);
            }
            let request_id = request.request_id.clone();
            self.commit(Input::Command { identity, request }, now as u64)?;
            encode(&Receipt {
                request_id,
                version: self.current()?.version,
                duplicate: false,
            })
        })())
    }
    pub fn view(&self, subject: &str) -> std::result::Result<String, String> {
        wire(
            self.current()
                .and_then(|room| encode(&reducer::view(room, subject)?)),
        )
    }
    pub fn events(
        &self,
        subject: &str,
        after: f64,
        limit: u32,
    ) -> std::result::Result<String, String> {
        wire((|| {
            let room = self.current()?;
            if room.member(subject).is_none() {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "join before reading event history",
                ));
            }
            let events: Vec<_> = self
                .records
                .iter()
                .filter(|r| r.sequence > after as u64)
                .take(limit.clamp(1, 100) as usize)
                .map(|r| reducer::batch(room, subject, r))
                .collect::<Result<_>>()?;
            encode(&EventPage {
                game_id: room.id.clone(),
                next_after: events.last().map_or(after as u64, |e| e.sequence),
                events,
                current_version: room.version,
                archive_pending: false,
            })
        })())
    }
    pub fn presence(
        &mut self,
        subject: String,
        connected: bool,
        now: f64,
    ) -> std::result::Result<(), String> {
        wire((|| {
            let member = self
                .current()?
                .member(&subject)
                .ok_or_else(|| Error::new(ErrorCode::Forbidden, "join this game first"))?;
            if member.connected == connected {
                return Ok(());
            }
            self.commit(Input::Presence { subject, connected }, now as u64)
        })())
    }
    pub fn tick(&mut self, now: f64) -> std::result::Result<bool, String> {
        wire((|| {
            if self
                .room
                .as_ref()
                .and_then(Room::next_deadline_ms)
                .is_none_or(|d| d > now as u64)
            {
                return Ok(false);
            }
            self.commit(Input::Timeout, now as u64)?;
            Ok(true)
        })())
    }
    pub fn topology() -> std::result::Result<String, String> {
        wire(encode(settlers_engine::topology()))
    }
    pub fn checkpoint(&self) -> std::result::Result<String, String> {
        wire(encode(self))
    }
    /// Export the complete replay through victory to the local human host only.
    /// This unverified research deposit never grants hosted reputation or game access.
    pub fn completed_history(&self, subject: &str) -> std::result::Result<String, String> {
        wire((|| {
            let room = self.current()?;
            if subject != room.host || room.seat(subject).is_none() {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "only the local host can export",
                ));
            }
            let Some(settlers_engine::State {
                phase: settlers_engine::Phase::Finished { winner },
                ..
            }) = &room.game
            else {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "finish the game before exporting",
                ));
            };
            // Ignore later presence changes so retries and reopened results have one identity.
            let end = self
                .records
                .iter()
                .position(|record| {
                    record.public.iter().any(|event| {
                        matches!(
                            event,
                            PublicEvent::Rule {
                                event: settlers_engine::Event::Won { .. }
                            }
                        )
                    })
                })
                .ok_or_else(|| {
                    Error::new(ErrorCode::InvalidRequest, "victory record is missing")
                })?
                + 1;
            encode(&serde_json::json!({
                "schema": 1, "source": "local-unverified", "game_id": room.id,
                "winner": winner, "records": &self.records[..end],
            }))
        })())
    }
    /// Resume the saved turn with its remaining duration; closed tabs do not run clocks.
    pub fn restore(
        checkpoint: &str,
        paused_at: f64,
        now: f64,
    ) -> std::result::Result<BrowserRoom, String> {
        wire((|| {
            let mut saved: Self = decode(checkpoint)?;
            if let Some(room) = &mut saved.room {
                let elapsed = (now as u64).saturating_sub(paused_at as u64);
                room.deadline_ms = room.deadline_ms.map(|d| d.saturating_add(elapsed));
                if let Some(discard) = &mut room.discard {
                    discard.deadline_ms = discard.deadline_ms.saturating_add(elapsed);
                }
                for offer in &mut room.offers {
                    if offer.public.status == settlers_protocol::OfferStatus::Open {
                        offer.public.created_at_ms = offer
                            .public
                            .created_at_ms
                            .map(|d| d.saturating_add(elapsed));
                        offer.public.expires_at_ms = offer
                            .public
                            .expires_at_ms
                            .map(|d| d.saturating_add(elapsed));
                    }
                }
                for member in &mut room.members {
                    member.connected = false;
                }
            }
            Ok(saved)
        })())
    }
}
