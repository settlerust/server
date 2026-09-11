//! Predicting another seat's next build from public information. The prediction mirrors the
//! builders' own greedy target choice: given the public board and a hand (the seat's sampled
//! hand inside the search, or a public-knowledge estimate from outside it), it names the next
//! piece that seat will complete, the site that line ends up taking, and how soon the seat can
//! pay for it. The denial leaf term, the prediction-guided opponent model, and the predicted
//! `space` threat all read this one function.
use super::opponent::{self, expansion};
use settlers_engine::*;

/// A predicted build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Build {
    /// The next piece the seat will complete toward its target.
    pub action: Action,
    /// What that piece costs.
    pub cost: Resources,
    /// Turns until the next piece is affordable at the seat's production, per own turn.
    pub eta: f64,
    /// The vertex the line ends up building on when it takes one: the expansion site for a
    /// settlement line, the upgraded settlement for a city.
    pub site: Option<u8>,
    /// Turns until the whole line (the remaining roads plus the settlement, or the city) is
    /// affordable at the seat's production. The site falls to the seat around then.
    pub site_eta: f64,
}

/// Turns until `cost` is affordable from `hand` at seat `q`'s production per own turn.
fn eta(s: &State, q: u8, hand: Resources, cost: Resources) -> f64 {
    let rates = super::leaf::rates(s, q);
    let per_turn = f64::from(s.settings.players);
    (0..5)
        .map(|r| f64::from(cost.0[r].saturating_sub(hand.0[r])) / (rates[r] * per_turn + 0.03))
        .fold(0.0, f64::max)
}

/// The seat's next build, predicted from `hand` and the public board. `None` when the seat
/// has no pieces left to build. The site of a settlement line is only meaningful while the
/// vertex is still open.
pub fn next_build(s: &State, q: u8, hand: Resources) -> Option<Build> {
    let own = s.players[q as usize];
    if own.settlements >= 5 && own.cities >= 4 {
        return None;
    }
    let (action, cost) = opponent::target_for(s, q, hand);
    let action = action?;
    let mut site = None;
    let mut site_cost = cost;
    match action {
        Action::BuildSettlement { vertex } => site = Some(vertex),
        Action::BuildRoad { .. } => {
            // The road is the first piece of the expansion line; the site is where it ends.
            if let Some((vertex, _, d)) = expansion(s, q) {
                site = Some(vertex);
                site_cost = Resources::SETTLEMENT;
                // The whole line pays for the settlement plus the roads still to build.
                site_cost.0[0] = site_cost.0[0].saturating_add(d);
                site_cost.0[1] = site_cost.0[1].saturating_add(d);
            }
        }
        Action::BuildCity { vertex } => site = Some(vertex),
        _ => {}
    }
    Some(Build {
        action,
        cost,
        eta: eta(s, q, hand, cost),
        site,
        site_eta: eta(s, q, hand, site_cost),
    })
}

/// The prediction-guided action for seat `q` in world `s`: complete the predicted piece when
/// it is affordable, otherwise take the cheapest step toward it (a bank trade that strictly
/// reduces its deficit), otherwise end the turn and save. Setup, discards, robber, and dev
/// card plays keep the builder's behaviour. `steps` counts main-phase actions this turn.
pub fn act(s: &State, q: u8, steps: u32) -> Action {
    let own = s.players[q as usize];
    match s.phase {
        Phase::SetupSettlement { .. }
        | Phase::SetupRoad { .. }
        | Phase::Discard { .. }
        | Phase::Robber { .. }
        | Phase::FreeRoads { .. }
        | Phase::Finished { .. } => return opponent::act(s, q, steps),
        Phase::Roll | Phase::Main => {}
    }
    let can_play = !s.played_development;
    if can_play && own.development[Development::Knight as usize] > 0 && steps < 16 {
        return Action::PlayKnight;
    }
    if s.phase == Phase::Roll {
        return Action::Roll;
    }
    if steps >= 16 {
        return Action::EndTurn;
    }
    let Some(build) = next_build(s, q, own.resources) else {
        return opponent::act(s, q, steps);
    };
    if apply(s, q, build.action).is_ok() {
        return build.action;
    }
    // A bank trade toward the predicted piece, when surplus cards strictly reduce its deficit.
    let mut best: Option<(u16, Action)> = None;
    for give_r in 0..5 {
        let ratio = trade_ratio(s, q, Resource::ALL[give_r]);
        let surplus = own.resources.0[give_r].saturating_sub(build.cost.0[give_r]);
        if surplus < ratio {
            continue;
        }
        for get_r in 0..5 {
            if get_r == give_r || s.bank.0[get_r] == 0 {
                continue;
            }
            let after = Resources(std::array::from_fn(|r| {
                own.resources.0[r] - if r == give_r { ratio } else { 0 } + u8::from(r == get_r)
            }));
            let before = crate::leaf::deficit(own.resources, build.cost) as u16;
            let later = crate::leaf::deficit(after, build.cost) as u16;
            if later < before {
                let action = Action::BankTrade {
                    give: Resources::one(Resource::ALL[give_r], ratio),
                    receive: Resources::one(Resource::ALL[get_r], 1),
                };
                if best.as_ref().is_none_or(|b| later < b.0) {
                    best = Some((later, action));
                }
            }
        }
    }
    if let Some((_, action)) = best
        && apply(s, q, action).is_ok()
    {
        return action;
    }
    Action::EndTurn
}
