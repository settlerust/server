use crate::{Communication, Filter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settlers_engine::{Observation, Resources, Settings};
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemberView {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disconnected_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<crate::DisconnectResolution>,
    pub seat: Option<u8>,
    pub name: String,
    pub connected: bool,
    #[serde(default)]
    pub ready: bool,
    pub is_you: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OfferStatus {
    Open,
    Accepted { player: u8 },
    Cancelled,
    Expired,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TradeOffer {
    pub id: String,
    pub from: u8,
    pub to: Vec<u8>,
    pub give: Resources,
    pub receive: Resources,
    pub message: Option<Communication>,
    pub status: OfferStatus,
    /// Explicit passes only; private automatic filtering is never disclosed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declined: Vec<u8>,
    /// Explicit consideration only; independent of resource holdings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GameView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub research: bool,
    /// Only research spectators receive hands; seated agents still have normal redaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_hands: Option<Vec<ResearchHand>>,
    pub version: u64,
    pub settings: Settings,
    pub host: bool,
    pub members: Vec<MemberView>,
    pub game: Option<Observation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening: Option<Opening>,
    pub offers: Vec<TradeOffer>,
    pub filters: Option<[Filter; 4]>,
    pub deadline_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard: Option<DiscardView>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResearchHand {
    pub seat: u8,
    pub hand: settlers_engine::Player,
}
/// Public opening rolls. Only tied leaders roll again; play proceeds clockwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Opening {
    pub rolls: Vec<OpeningRoll>,
    pub first: u8,
    pub order: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpeningRoll {
    pub player: u8,
    pub first: u8,
    pub second: u8,
    pub round: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GameSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub research: bool,
    pub version: u64,
    pub players: u8,
    pub capacity: u8,
    pub started: bool,
    #[serde(default)]
    pub connected_players: u8,
    pub finished: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GameList {
    pub games: Vec<GameSummary>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlayersView {
    pub members: Vec<MemberView>,
    pub players: Vec<settlers_engine::PublicPlayer>,
    pub own: Option<settlers_engine::Player>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub status: HealthStatus,
    pub protocol: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
}

/// Private choices for the receiving seat. Other seats and spectators never receive them.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscardView {
    pub id: u64,
    pub required: u16,
    pub locked: Resources,
    pub draft: Resources,
    /// Zero denotes an untimed discard round.
    pub deadline_ms: u64,
}
