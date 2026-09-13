use crate::auth::Identity;
use serde::{Deserialize, Serialize};
use settlers_engine::{Settings, State};
use settlers_protocol::*;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub identity: Identity,
    pub seat: Option<u8>,
    pub connected: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ready: bool,
    pub grace_turn: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredReceipt {
    pub subject: String,
    pub request: CommandRequest,
    pub version: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offer {
    pub public: TradeOffer,
    pub rejected: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub schema: u8,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub research: bool,
    pub host: String,
    pub version: u64,
    pub settings: Settings,
    pub seed: [u8; 32],
    pub members: Vec<Member>,
    pub game: Option<State>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening: Option<Opening>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<AdmissionUpdate>,
    pub offers: Vec<Offer>,
    pub filters: [[Filter; 4]; 4],
    pub receipts: Vec<StoredReceipt>,
    pub deadline_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard: Option<DiscardRound>,
    pub chat_window_ms: [u64; 4],
    pub chat_count: [u8; 4],
}
impl Room {
    /// Earliest authoritative clock, including offers while the turn remains open.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.deadline_ms
            .into_iter()
            .chain(
                self.offers
                    .iter()
                    .filter(|offer| offer.public.status == OfferStatus::Open)
                    .filter_map(|offer| offer.public.expires_at_ms),
            )
            .min()
    }
    pub fn member(&self, subject: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.identity.subject == subject)
    }
    pub fn seat(&self, subject: &str) -> Option<u8> {
        self.member(subject).and_then(|m| m.seat)
    }
    pub fn summary(&self) -> GameSummary {
        GameSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            research: self.research,
            version: self.version,
            players: self.members.iter().filter(|m| m.seat.is_some()).count() as u8,
            capacity: self.settings.players,
            started: self.game.is_some(),
            connected_players: self
                .members
                .iter()
                .filter(|m| m.seat.is_some() && m.connected)
                .count() as u8,
            finished: self
                .game
                .is_some_and(|s| matches!(s.phase, settlers_engine::Phase::Finished { .. })),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    Admission {
        identity: Identity,
        request: AdmissionUpdate,
    },
    Create {
        request: CreateGame,
        identity: Identity,
        seed: [u8; 32],
    },
    Command {
        identity: Identity,
        request: CommandRequest,
    },
    Presence {
        subject: String,
        connected: bool,
    },
    Timeout,
}
/// Private research archive. Participants only receive its redacted `public` projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub schema: u8,
    pub engine_version: String,
    pub game_id: String,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub input: Input,
    pub public: Vec<PublicEvent>,
    pub state_hash: String,
}

/// Server-owned staging; the engine receives one exact, fully resolved discard per seat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscardRound {
    pub id: u64,
    pub required: [u16; 4],
    pub locked: [Resources; 4],
    pub drafts: [Resources; 4],
    pub deadline_ms: u64,
    pub turn_remaining_ms: u64,
}
