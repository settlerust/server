use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PublicPlayer {
    pub seat: u8,
    pub resource_count: u16,
    pub development_count: u8,
    pub played_knights: u8,
    pub settlements: u8,
    pub cities: u8,
    pub roads: u8,
    pub points: u8,
    /// Revealed for every player only once the game has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub victory_cards: Option<u8>,
    pub road_length: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    pub settings: Settings,
    pub buildings: Vec<u8>,
    pub roads: Vec<Option<u8>>,
    pub hexes: [u8; 19],
    pub ports: [u8; 9],
    pub robber: u8,
    /// Exact public bank inventory in brick, lumber, wool, grain, ore order.
    pub bank: Resources,
    pub bank_available: [bool; 5],
    pub deck_count: u8,
    pub players: Vec<PublicPlayer>,
    pub own: Option<Player>,
    pub seat: Option<u8>,
    pub phase: Phase,
    pub current: u8,
    pub turn: u32,
    pub longest_road: Option<u8>,
    pub largest_army: Option<u8>,
    pub played_development: bool,
    pub legal_actions: Vec<Action>,
}
/// Per-seat observation; spectators receive only public information.
pub fn observe(s: &State, seat: Option<u8>) -> Observation {
    let seat = seat.filter(|&p| p < s.settings.players);
    Observation {
        settings: s.settings,
        buildings: (0..54).map(|v| s.board.building(v)).collect(),
        roads: (0..72).map(|e| s.board.road(e)).collect(),
        hexes: s.layout.hexes,
        ports: s.layout.ports,
        robber: s.robber,
        bank: s.bank,
        bank_available: s.bank.0.map(|count| count > 0),
        deck_count: s.deck_count(),
        players: (0..s.settings.players)
            .map(|p| {
                let player = s.players[p as usize];
                PublicPlayer {
                    seat: p,
                    resource_count: player.resources.total(),
                    development_count: player
                        .development
                        .iter()
                        .chain(player.new_development.iter())
                        .sum(),
                    played_knights: player.played_knights,
                    settlements: player.settlements,
                    cities: player.cities,
                    roads: player.roads,
                    points: if matches!(s.phase, Phase::Finished { .. }) {
                        points(s, p)
                    } else {
                        public_points(s, p)
                    },
                    road_length: road_length(s, p),
                    victory_cards: matches!(s.phase, Phase::Finished { .. }).then_some(
                        player.development[Development::VictoryPoint as usize]
                            + player.new_development[Development::VictoryPoint as usize],
                    ),
                }
            })
            .collect(),
        own: seat.map(|p| s.players[p as usize]),
        seat,
        phase: s.phase,
        current: s.current,
        turn: s.turn,
        longest_road: s.longest_road,
        largest_army: s.largest_army,
        played_development: s.played_development,
        legal_actions: seat.map_or_else(Vec::new, |p| legal_actions(s, p)),
    }
}
