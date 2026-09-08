use crate::*;
/// An exact stochastic branch for offline search over an authoritative or sampled state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightedChance {
    pub outcome: Chance,
    pub numerator: u32,
    pub denominator: u32,
}
/// Enumerate dice, weighted resource theft, or development draws. Hidden state must be
/// sampled from a belief before calling this in a fair imperfect-information policy.
pub fn chance_outcomes(
    state: &State,
    player: u8,
    action: Action,
) -> Result<Vec<WeightedChance>, RuleError> {
    state.player(player)?;
    let mut outcomes = vec![];
    match action {
        Action::Roll => {
            for first in 1..=6 {
                for second in 1..=6 {
                    outcomes.push(WeightedChance {
                        outcome: Chance::Dice { first, second },
                        numerator: 1,
                        denominator: 36,
                    });
                }
            }
        }
        Action::BuyDevelopment => {
            let denominator = state.deck_count() as u32;
            for card in Development::ALL {
                let numerator = state.deck[card as usize] as u32;
                if numerator > 0 {
                    outcomes.push(WeightedChance {
                        outcome: Chance::Development { card },
                        numerator,
                        denominator,
                    });
                }
            }
        }
        Action::MoveRobber {
            victim: Some(victim),
            ..
        } => {
            let hand = state.player(victim)?.resources;
            for resource in Resource::ALL {
                let numerator = hand.0[resource as usize] as u32;
                if numerator > 0 {
                    outcomes.push(WeightedChance {
                        outcome: Chance::Steal { resource },
                        numerator,
                        denominator: hand.total() as u32,
                    });
                }
            }
        }
        _ => return Err(RuleError::InvalidChance),
    }
    for branch in &outcomes {
        apply_with_chance(state, player, action, branch.outcome)?;
    }
    if outcomes.is_empty() {
        return Err(RuleError::InvalidChance);
    }
    Ok(outcomes)
}
