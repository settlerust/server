//! Persisted rooms are private, but their bytes are still a fallible input boundary.
use super::Room;
use crate::error::{Error, Result};
use settlers_protocol::ErrorCode;
use std::collections::BTreeSet;

fn require(valid: bool, message: &'static str) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::Internal,
            format!("invalid persisted room: {message}"),
        ))
    }
}
impl Room {
    /// Validate before recovery, projection, or replay can index seat-specific data.
    pub fn validate(&self) -> Result<()> {
        require(self.schema == 1 && self.version > 0, "schema or version")?;
        self.settings.validate()?;
        let mut subjects = BTreeSet::new();
        let mut seats = BTreeSet::new();
        for member in &self.members {
            require(
                subjects.insert(&member.identity.subject),
                "duplicate identity",
            )?;
            if let Some(seat) = member.seat {
                require(
                    seat < self.settings.players && seats.insert(seat),
                    "invalid or duplicate seat",
                )?;
            }
        }
        if let Some(game) = &self.game {
            require(game.settings == self.settings, "settings mismatch")?;
            game.validate()
                .map_err(|error| Error::new(ErrorCode::Internal, error.to_string()))?;
        }
        if let Some(opening) = &self.opening {
            require(opening.first < self.settings.players, "opening first seat")?;
            require(
                opening.order.len() == self.settings.players as usize
                    && opening.order.iter().copied().collect::<BTreeSet<_>>()
                        == (0..self.settings.players).collect(),
                "opening order",
            )?;
            require(
                opening.rolls.iter().all(|r| {
                    r.player < self.settings.players
                        && (1..=6).contains(&r.first)
                        && (1..=6).contains(&r.second)
                }),
                "opening rolls",
            )?;
        }
        for offer in &self.offers {
            require(
                offer.public.from < self.settings.players
                    && offer
                        .public
                        .to
                        .iter()
                        .chain(&offer.public.declined)
                        .chain(&offer.public.thinking)
                        .all(|&p| p < self.settings.players),
                "offer seats",
            )?;
        }
        Ok(())
    }
}
