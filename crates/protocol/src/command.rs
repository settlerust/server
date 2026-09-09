use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settlers_engine::{Action, Resources, Settings};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuestRequest {
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TokenResponse {
    pub token: String,
    pub subject: String,
    pub expires_at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGame {
    pub name: String,
    /// Agent-only table whose spectators can inspect every player's hand.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub research: bool,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default = "yes")]
    pub host_as_player: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_name: Option<String>,
}
fn yes() -> bool {
    true
}

/// Control-plane admission fence. Accepted only from an external host grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdmissionUpdate {
    pub revision: u64,
    pub players: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub mute_chat: bool,
    pub mute_trade_messages: bool,
    pub auto_reject_trades: bool,
    pub shadow_mute: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Chat,
    BuildRoad,
    BuildSettlement,
    BuildCity,
    BuyDevelopment,
    RequestTrade,
    Strategy,
}
/// A policy can inspect intent/resources without parsing the optional natural-language text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Communication {
    pub intent: Intent,
    #[serde(default)]
    pub resources: Option<Resources>,
    #[serde(default)]
    pub text: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Explicitly enroll an agent; required for player seats in research tables.
    JoinAgent {
        name: String,
    },
    Join {
        name: String,
        #[serde(default)]
        spectator: bool,
    },
    /// Confirm or withdraw this player's readiness while connected by WebSocket.
    SetReady {
        ready: bool,
    },
    /// Any seated player (or observer host) can start when all seats are ready and connected.
    Start,
    /// Roll publicly for the first seat once every player is ready.
    RollForOrder,
    Configure {
        settings: Settings,
    },
    Act {
        action: Action,
    },
    /// Replace the private, editable selection for this discard round.
    SelectDiscard {
        discard_id: u64,
        resources: Resources,
    },
    /// Lock the current selection; further selections may fill the remaining quota.
    ConfirmDiscard {
        discard_id: u64,
    },
    OfferTrade {
        to: Vec<u8>,
        give: Resources,
        receive: Resources,
        #[serde(default)]
        message: Option<Communication>,
    },
    AcceptTrade {
        offer_id: String,
    },
    RejectTrade {
        offer_id: String,
    },
    CancelTrade {
        offer_id: String,
    },
    Communicate {
        #[serde(default)]
        to: Option<Vec<u8>>,
        message: Communication,
    },
    SetFilter {
        player: u8,
        filter: Filter,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub request_id: String,
    pub expected_version: u64,
    pub command: Command,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Command { request: CommandRequest },
    Ping { nonce: String },
    Resync,
}
