//! Validate serialized state before any rule code indexes its compact arrays.
use crate::{Phase, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid game state: {0}")]
pub struct StateValidationError(pub &'static str);

type Result = std::result::Result<(), StateValidationError>;
fn require(valid: bool, message: &'static str) -> Result {
    if valid {
        Ok(())
    } else {
        Err(StateValidationError(message))
    }
}

impl State {
    /// Check structural indices and conserved inventories at a persistence boundary.
    /// Rule transitions remain allocation-free and do not repeat this whole-state scan.
    pub fn validate(&self) -> Result {
        require(self.settings.validate().is_ok(), "settings")?;
        let seats = self.settings.players;
        let mask = (1u8 << seats) - 1;
        require(self.current < seats && self.first < seats, "turn seats")?;
        require(self.robber < 19, "robber hex")?;
        require(
            self.layout.hexes.iter().all(|&hex| {
                hex & 15 <= 5
                    && (hex >> 4 == 0 || ((2..=12).contains(&(hex >> 4)) && hex >> 4 != 7))
            }) && self.layout.ports.iter().all(|&port| port <= 5),
            "layout encoding",
        )?;
        require(
            self.retired & !mask == 0 && !self.is_retired(self.current),
            "retired seats",
        )?;
        for award in [self.longest_road, self.largest_army].into_iter().flatten() {
            require(award < seats && !self.is_retired(award), "award seat")?;
        }
        match self.phase {
            Phase::SetupSettlement { step } => require(step < 2 * seats, "setup step")?,
            Phase::SetupRoad { step, vertex } => {
                require(step < 2 * seats && vertex < 54, "setup road")?;
            }
            Phase::Discard { pending } => {
                require(
                    pending != 0 && pending & (!mask | self.retired) == 0,
                    "discard seats",
                )?;
            }
            Phase::FreeRoads { remaining, .. } => {
                require((1..=2).contains(&remaining), "free roads")?
            }
            Phase::Finished { winner } => {
                require(winner < seats && !self.is_retired(winner), "winner")?
            }
            _ => {}
        }
        let mut pieces = [[0u8; 3]; 4];
        for vertex in 0..54 {
            let value = self.board.building(vertex);
            require(value <= 8, "packed building")?;
            if value != 0 {
                let seat = (value - 1) % 4;
                require(seat < seats, "building seat")?;
                pieces[seat as usize][usize::from(value > 4)] += 1;
            }
        }
        for edge in 0..72 {
            if let Some(seat) = self.board.road(edge) {
                require(seat < seats, "road seat")?;
                pieces[seat as usize][2] += 1;
            }
        }
        for (player, pieces) in self.players.iter().zip(pieces) {
            require(
                [player.settlements, player.cities, player.roads] == pieces,
                "piece inventory",
            )?;
            require(
                player.settlements <= 5 && player.cities <= 4 && player.roads <= 15,
                "piece limits",
            )?;
        }
        for resource in 0..5 {
            let total = u16::from(self.bank.0[resource])
                + self
                    .players
                    .iter()
                    .map(|p| u16::from(p.resources.0[resource]))
                    .sum::<u16>();
            require(total == 19, "resource inventory")?;
        }
        for (card, initial) in [14u16, 2, 2, 2, 5].into_iter().enumerate() {
            let total = u16::from(self.deck[card])
                + u16::from(self.used_development[card])
                + self
                    .players
                    .iter()
                    .map(|p| u16::from(p.development[card]) + u16::from(p.new_development[card]))
                    .sum::<u16>();
            require(total == initial, "development inventory")?;
        }
        for player in &self.players[seats as usize..] {
            require(
                *player == crate::Player::default(),
                "inactive player inventory",
            )?;
        }
        Ok(())
    }
}
