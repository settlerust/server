use crate::{Communication, GameView, TradeOffer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settlers_engine::{Event, RuleError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublicEvent {
    LobbyCreated,
    SettingsChanged {
        settings: crate::Settings,
    },
    Joined {
        seat: Option<u8>,
        name: String,
    },
    Started,
    OrderDecided {
        opening: crate::Opening,
    },
    Presence {
        seat: Option<u8>,
        name: String,
        connected: bool,
    },
    Rule {
        event: Event,
    },
    /// Only the thief and victim receive the resource identity.
    TheftDetails {
        player: u8,
        victim: u8,
        resource: settlers_engine::Resource,
    },
    TradeOffered {
        offer: TradeOffer,
    },
    TradeClosed {
        offer_id: String,
        reason: TradeCloseReason,
        winner: Option<u8>,
    },
    TradeRejected {
        offer_id: String,
        player: u8,
    },
    Message {
        from: u8,
        to: Option<Vec<u8>>,
        message: Communication,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TradeCloseReason {
    Accepted,
    Cancelled,
    TurnEnded,
    Replaced,
    ResourcesChanged,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EventPage {
    pub game_id: String,
    pub events: Vec<EventBatch>,
    pub next_after: u64,
    pub current_version: u64,
    pub archive_pending: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventBatch {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub events: Vec<PublicEvent>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    NotFound,
    InvalidRequest,
    Conflict,
    RuleViolation,
    TradeTaken,
    TradeUnavailable,
    StorageUnavailable,
    Backpressure,
    Internal,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<u8>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Receipt {
    pub request_id: String,
    pub version: u64,
    pub duplicate: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Snapshot {
        protocol: u8,
        game: Box<GameView>,
    },
    Events {
        batch: EventBatch,
    },
    Ack {
        receipt: Receipt,
    },
    Error {
        request_id: Option<String>,
        error: ApiError,
    },
    Pong {
        nonce: String,
    },
    ResyncRequired,
}
