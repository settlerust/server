use crate::leaf;
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{Command, Communication, GameView, Intent, OfferStatus};

pub struct Decision {
    pub command: Command,
    pub action: Action,
    pub offer: bool,
}
pub fn candidates(
    view: &GameView,
    k: &ResourceKnowledge,
    offered_turn: Option<u32>,
) -> Vec<Decision> {
    let Some(o) = &view.game else {
        return vec![];
    };
    let (Some(p), Some(own)) = (o.seat, o.own) else {
        return vec![];
    };
    let mut result: Vec<_> = o
        .legal_actions
        .iter()
        .copied()
        .filter(|a| !matches!(a, Action::RelocateSettlement { .. } | Action::Trade { .. }))
        .map(|action| Decision {
            command: Command::Act { action },
            action,
            offer: false,
        })
        .collect();
    if o.phase != Phase::Main {
        return result;
    }
    for offer in &view.offers {
        if offer.status == OfferStatus::Open
            && offer.to.contains(&p)
            && own.resources.contains(offer.receive)
            && (o.current == p || o.current == offer.from)
        {
            result.push(Decision {
                command: Command::AcceptTrade {
                    offer_id: offer.id.clone(),
                },
                action: Action::Trade {
                    other: offer.from,
                    give: offer.receive,
                    receive: offer.give,
                },
                offer: false,
            });
        }
    }
    if o.current != p
        || offered_turn == Some(o.turn)
        || view.offers.iter().any(|offer| offer.from == p)
    {
        return result;
    }
    // Enumerate 1:1 and 2:1 swaps toward every build cost, including buildings not yet affordable.
    for other in 0..o.settings.players {
        if other == p {
            continue;
        }
        for cost in [
            Resources::ROAD,
            Resources::SETTLEMENT,
            Resources::CITY,
            Resources::DEVELOPMENT,
        ] {
            for wanted in Resource::ALL {
                let r = wanted as usize;
                if own.resources.0[r] >= cost.0[r] {
                    continue;
                }
                let receive = Resources::one(wanted, 1);
                if !k.players[other as usize].can_have(receive) {
                    continue;
                }
                for surplus in Resource::ALL {
                    if surplus == wanted {
                        continue;
                    }
                    for n in 1..=2 {
                        if own.resources.0[surplus as usize] < cost.0[surplus as usize] + n {
                            continue;
                        }
                        let give = Resources::one(surplus, n);
                        let action = Action::Trade {
                            other,
                            give,
                            receive,
                        };
                        if result.iter().any(|d| d.offer && d.action == action) {
                            continue;
                        }
                        result.push(Decision {
                            command: Command::OfferTrade {
                                to: vec![other],
                                give,
                                receive,
                                expires_in_ms: None,
                                message: Some(Communication {
                                    intent: Intent::RequestTrade,
                                    resources: Some(receive),
                                    text: Some("Would this swap work for your next build?".into()),
                                }),
                            },
                            action,
                            offer: true,
                        });
                    }
                }
            }
        }
    }
    result
}

/// Explicit baseline assumption, never an authoritative prediction of another player's choice.
pub fn acceptance(s: &State, p: u8, action: Action) -> f64 {
    let Action::Trade {
        other,
        give,
        receive,
    } = action
    else {
        return 1.0;
    };
    let hand = s.players[other as usize].resources;
    if !hand.contains(receive) {
        return 0.0;
    }
    let after = Resources(std::array::from_fn(|r| {
        hand.0[r] - receive.0[r] + give.0[r]
    }));
    let improvement = [
        Resources::CITY,
        Resources::SETTLEMENT,
        Resources::DEVELOPMENT,
        Resources::ROAD,
    ]
    .iter()
    .map(|&cost| leaf::deficit(hand, cost) - leaf::deficit(after, cost))
    .fold(f64::NEG_INFINITY, f64::max);
    let threat = if public_points(s, p) + 2 >= s.settings.points_to_win {
        0.5
    } else {
        1.0
    };
    (if improvement > 0.0 {
        0.7
    } else if give.total() > receive.total() {
        0.45
    } else {
        0.2
    }) * threat
}

pub fn unlocked(before: &State, after: &State, p: u8) -> Vec<Action> {
    let original = legal_actions(before, p);
    legal_actions(after, p)
        .into_iter()
        .filter(|a| {
            matches!(
                a,
                Action::BuildCity { .. }
                    | Action::BuildSettlement { .. }
                    | Action::BuildRoad { .. }
                    | Action::BuyDevelopment
            ) && !original.contains(a)
        })
        .collect()
}
