pub use crate::build::{road_allowed, settlement_allowed};
pub use crate::economy::trade_ratio;
use crate::*;

/// Finite legal suggestions. Includes one discard choice and unit bank trades;
/// clients can submit any legal quantities, discards, or negotiated trade.
pub fn legal_actions(s: &State, p: u8) -> Vec<Action> {
    if p >= s.settings.players || s.is_retired(p) || matches!(s.phase, Phase::Finished { .. }) {
        return vec![];
    }
    if let Phase::Discard { pending } = s.phase {
        return if pending & (1 << p) != 0 {
            vec![Action::Discard {
                resources: automatic_discard(s.players[p as usize].resources),
            }]
        } else {
            vec![]
        };
    }
    if p != s.current {
        return vec![];
    }
    let mut candidates = Vec::new();
    match s.phase {
        Phase::SetupSettlement { .. } => {
            for vertex in 0..54 {
                candidates.push(Action::BuildSettlement { vertex });
            }
        }
        Phase::SetupRoad { .. } | Phase::FreeRoads { .. } => {
            for edge in 0..72 {
                candidates.push(Action::BuildRoad { edge });
            }
            candidates.push(Action::FinishRoadBuilding);
            if matches!(s.phase, Phase::SetupRoad { .. }) {
                for vertex in 0..54 {
                    candidates.push(Action::RelocateSettlement { vertex });
                }
            }
        }
        Phase::Robber { .. } => {
            for hex in 0..19 {
                let victims = robber_victims(s, p, hex);
                if victims.is_empty() {
                    candidates.push(Action::MoveRobber { hex, victim: None });
                } else {
                    for victim in victims {
                        candidates.push(Action::MoveRobber {
                            hex,
                            victim: Some(victim),
                        });
                    }
                }
            }
        }
        Phase::Roll => candidates.push(Action::Roll),
        Phase::Main => {
            candidates.push(Action::EndTurn);
            candidates.push(Action::BuyDevelopment);
            for vertex in 0..54 {
                candidates.push(Action::BuildSettlement { vertex });
                candidates.push(Action::BuildCity { vertex });
            }
            for edge in 0..72 {
                candidates.push(Action::BuildRoad { edge });
            }
            for r in Resource::ALL {
                for other in Resource::ALL {
                    if r != other {
                        candidates.push(Action::BankTrade {
                            give: Resources::one(r, trade_ratio(s, p, r)),
                            receive: Resources::one(other, 1),
                        });
                    }
                }
            }
        }
        _ => {}
    }
    if matches!(s.phase, Phase::Roll | Phase::Main) {
        candidates.push(Action::PlayKnight);
        candidates.push(Action::PlayRoadBuilding);
        for resource in Resource::ALL {
            candidates.push(Action::PlayMonopoly { resource });
        }
        let count = s.bank.total().min(2);
        if count == 0 {
            candidates.push(Action::PlayYearOfPlenty {
                resources: Resources::default(),
            });
        }
        for i in 0..5 {
            for j in i..5 {
                let mut resources = Resources::default();
                resources.0[i] += 1;
                if count == 2 {
                    resources.0[j] += 1;
                }
                candidates.push(Action::PlayYearOfPlenty { resources });
                if count < 2 {
                    break;
                }
            }
        }
    }
    candidates.retain(|&a| apply(s, p, a).is_ok());
    candidates
}
/// Deterministic timeout fallback: discard from the largest stack, ties by resource ID.
pub fn automatic_discard(mut hand: Resources) -> Resources {
    let count = hand.total() / 2;
    let mut discard = Resources::default();
    for _ in 0..count {
        let r = (0..5).max_by_key(|&r| (hand.0[r], 5 - r)).unwrap_or(0);
        hand.0[r] -= 1;
        discard.0[r] += 1;
    }
    discard
}
