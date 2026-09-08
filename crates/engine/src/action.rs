use crate::{Development, Phase, Resource, Resources};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Correct the current setup settlement before confirming its connecting road.
    RelocateSettlement {
        vertex: u8,
    },
    BuildSettlement {
        vertex: u8,
    },
    BuildRoad {
        edge: u8,
    },
    BuildCity {
        vertex: u8,
    },
    Roll,
    Discard {
        resources: Resources,
    },
    MoveRobber {
        hex: u8,
        victim: Option<u8>,
    },
    BuyDevelopment,
    PlayKnight,
    PlayRoadBuilding,
    /// Decline the optional second road after placing the first free road.
    FinishRoadBuilding,
    PlayMonopoly {
        resource: Resource,
    },
    PlayYearOfPlenty {
        resources: Resources,
    },
    BankTrade {
        give: Resources,
        receive: Resources,
    },
    /// Only the server may use this after resolving an authenticated offer acceptance.
    Trade {
        other: u8,
        give: Resources,
        receive: Resources,
    },
    EndTurn,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Chance {
    Dice { first: u8, second: u8 },
    Steal { resource: Resource },
    Development { card: Development },
}

/// Public rule effects. Hidden draws, discarded types, and stolen types are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SetupSettlementMoved {
        player: u8,
        from: u8,
        vertex: u8,
        returned: Resources,
        received: Resources,
    },
    BuiltSettlement {
        player: u8,
        vertex: u8,
    },
    BuiltRoad {
        player: u8,
        edge: u8,
    },
    BuiltCity {
        player: u8,
        vertex: u8,
    },
    Rolled {
        player: u8,
        first: u8,
        second: u8,
    },
    Produced {
        player: u8,
        resources: Resources,
    },
    Discarded {
        player: u8,
        count: u16,
    },
    RobberMoved {
        player: u8,
        hex: u8,
    },
    Stole {
        player: u8,
        victim: u8,
        count: u8,
    },
    BoughtDevelopment {
        player: u8,
    },
    PlayedDevelopment {
        player: u8,
        card: Development,
    },
    TookResources {
        player: u8,
        resources: Resources,
    },
    MonopolyCollected {
        player: u8,
        victim: u8,
        resource: Resource,
        count: u8,
    },
    BankTraded {
        player: u8,
        give: Resources,
        receive: Resources,
    },
    Traded {
        player: u8,
        other: u8,
        give: Resources,
        receive: Resources,
    },
    AwardsChanged {
        longest_road: Option<u8>,
        largest_army: Option<u8>,
    },
    TurnChanged {
        player: u8,
        turn: u32,
    },
    PhaseChanged {
        phase: Phase,
    },
    Won {
        player: u8,
        points: u8,
    },
    TimedOut {
        player: u8,
    },
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RuleError {
    #[error("invalid game settings")]
    InvalidSettings,
    #[error("invalid player seat")]
    InvalidPlayer,
    #[error("it is another player's turn")]
    NotYourTurn,
    #[error("action is not allowed in this phase")]
    WrongPhase,
    #[error("invalid board location")]
    InvalidLocation,
    #[error("building or road placement is not legal")]
    IllegalPlacement,
    #[error("not enough resources")]
    InsufficientResources,
    #[error("the bank cannot supply these resources")]
    BankShortage,
    #[error("no pieces of this type remain")]
    NoPieces,
    #[error("no development cards remain")]
    EmptyDeck,
    #[error("development card is unavailable, new, or already played this turn")]
    CannotPlayDevelopment,
    #[error("discard must contain exactly half the hand, rounded down")]
    InvalidDiscard,
    #[error("choose an adjacent opponent with resource cards")]
    InvalidVictim,
    #[error("invalid trade quantities, resources, or participants")]
    InvalidTrade,
    #[error("invalid chance outcome for this action")]
    InvalidChance,
    #[error("game has finished")]
    GameFinished,
}
