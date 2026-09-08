use crate::{Board, Layout, Random, RuleError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Resource {
    Brick,
    Lumber,
    Wool,
    Grain,
    Ore,
}
impl Resource {
    pub const ALL: [Self; 5] = [
        Self::Brick,
        Self::Lumber,
        Self::Wool,
        Self::Grain,
        Self::Ore,
    ];
}

/// Resource counts in brick, lumber, wool, grain, ore order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Resources(pub [u8; 5]);
impl Resources {
    pub const ROAD: Self = Self([1, 1, 0, 0, 0]);
    pub const SETTLEMENT: Self = Self([1, 1, 1, 1, 0]);
    pub const CITY: Self = Self([0, 0, 0, 2, 3]);
    pub const DEVELOPMENT: Self = Self([0, 0, 1, 1, 1]);
    pub fn total(self) -> u16 {
        self.0.iter().map(|&x| x as u16).sum()
    }
    pub fn contains(self, other: Self) -> bool {
        self.0.iter().zip(other.0).all(|(&a, b)| a >= b)
    }
    pub fn disjoint(self, other: Self) -> bool {
        self.0.iter().zip(other.0).all(|(&a, b)| a == 0 || b == 0)
    }
    pub(crate) fn add(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }
    pub(crate) fn subtract(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a -= b;
        }
    }
    pub fn one(resource: Resource, count: u8) -> Self {
        let mut r = Self::default();
        r.0[resource as usize] = count;
        r
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Development {
    Knight,
    RoadBuilding,
    YearOfPlenty,
    Monopoly,
    VictoryPoint,
}
impl Development {
    pub const ALL: [Self; 5] = [
        Self::Knight,
        Self::RoadBuilding,
        Self::YearOfPlenty,
        Self::Monopoly,
        Self::VictoryPoint,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub players: u8,
    pub points_to_win: u8,
    pub discard_limit: u8,
    pub turn_seconds: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            players: 4,
            points_to_win: 10,
            discard_limit: 7,
            turn_seconds: 120,
        }
    }
}
impl Settings {
    pub fn validate(self) -> Result<(), RuleError> {
        if !(2..=4).contains(&self.players)
            || !(3..=20).contains(&self.points_to_win)
            || self.discard_limit > 95
            || !(1..=86400).contains(&self.turn_seconds)
        {
            return Err(RuleError::InvalidSettings);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resume {
    Roll,
    Main,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Phase {
    SetupSettlement { step: u8 },
    SetupRoad { step: u8, vertex: u8 },
    Roll,
    Discard { pending: u8 },
    Robber { resume: Resume },
    Main,
    FreeRoads { remaining: u8, resume: Resume },
    Finished { winner: u8 },
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Player {
    pub resources: Resources,
    pub development: [u8; 5],
    pub new_development: [u8; 5],
    pub played_knights: u8,
    pub settlements: u8,
    pub cities: u8,
    pub roads: u8,
}
/// Authoritative hidden state. Never serialize this to a participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub board: Board,
    pub layout: Layout,
    pub players: [Player; 4],
    pub bank: Resources,
    pub deck: [u8; 5],
    pub used_development: [u8; 5],
    pub settings: Settings,
    pub random: Random,
    pub phase: Phase,
    pub current: u8,
    pub first: u8,
    pub robber: u8,
    pub played_development: bool,
    pub longest_road: Option<u8>,
    pub largest_army: Option<u8>,
    pub turn: u32,
}
impl State {
    pub fn new(settings: Settings, seed: [u8; 32]) -> Result<Self, RuleError> {
        settings.validate()?;
        let mut random = Random::new(seed);
        let layout = Layout::generate(&mut random);
        let first = random.below(settings.players as u32) as u8;
        Ok(Self {
            board: Board::default(),
            layout,
            players: [Player::default(); 4],
            bank: Resources([19; 5]),
            deck: [14, 2, 2, 2, 5],
            used_development: [0; 5],
            settings,
            random,
            phase: Phase::SetupSettlement { step: 0 },
            current: first,
            first,
            robber: layout.desert(),
            played_development: false,
            longest_road: None,
            largest_army: None,
            turn: 0,
        })
    }
    pub fn player(&self, seat: u8) -> Result<&Player, RuleError> {
        if seat >= self.settings.players {
            return Err(RuleError::InvalidPlayer);
        }
        Ok(&self.players[seat as usize])
    }
    pub fn deck_count(&self) -> u8 {
        self.deck.iter().sum()
    }
    pub(crate) fn resume(&mut self, resume: Resume) {
        self.phase = match resume {
            Resume::Roll => Phase::Roll,
            Resume::Main => Phase::Main,
        };
    }
    pub(crate) fn setup_player(&self, step: u8) -> u8 {
        (self.first
            + if step < self.settings.players {
                step
            } else {
                2 * self.settings.players - 1 - step
            })
            % self.settings.players
    }
}
