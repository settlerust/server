//! Bargaining for the tunable search: which seats are dangerous to trade with, what to
//! propose, how to answer an open offer (accept, counter, or decline with a reason), and the
//! table talk that goes with each. Everything reads the seat's own view, the public board,
//! the bounded resource knowledge, and the public event history.
use crate::leaf::{deficit, site_value};
use settlers_engine::*;
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{
    Command, Communication, EventBatch, GameView, Intent, OfferStatus, PublicEvent, TradeOffer,
};

/// How dangerous another seat is as a trading partner, each component in `[0, 1]`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Threat {
    /// Ahead in the race or close to the finish.
    pub race: f64,
    /// Competing for the same expansion sites.
    pub space: f64,
    /// Contesting the longest road or the largest army with this seat.
    pub awards: f64,
    pub total: f64,
}
/// Road distances from a seat's network to every intersection; foreign buildings block.
pub fn distances(s: &State, p: u8) -> [u8; 54] {
    let t = topology();
    let mut distance = [u8::MAX; 54];
    let mut queue = Vec::with_capacity(54);
    for v in 0..54u8 {
        if s.board.owner(v) == Some(p)
            || (s.board.building(v) == 0
                && t.incident_edges[v as usize]
                    .iter()
                    .any(|&e| s.board.road(e) == Some(p)))
        {
            distance[v as usize] = 0;
            queue.push(v);
        }
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        let v = queue[cursor];
        cursor += 1;
        for &e in &t.incident_edges[v as usize] {
            if s.board.road(e).is_some() {
                continue;
            }
            let [a, b] = t.edges[e as usize];
            let next = if a == v { b } else { a };
            if s.board.owner(next).is_some_and(|o| o != p) {
                continue;
            }
            if distance[next as usize] > distance[v as usize] + 1 {
                distance[next as usize] = distance[v as usize] + 1;
                queue.push(next);
            }
        }
    }
    distance
}
/// The seat's best three reachable sites (value, vertex, distance) within three roads.
pub fn frontier(s: &State, p: u8, distance: &[u8; 54]) -> Vec<(f64, u8, u8)> {
    let mut sites: Vec<(f64, u8, u8)> = (0..54u8)
        .filter(|&v| distance[v as usize] <= 3 && settlement_allowed(s, p, v, true))
        .map(|v| {
            (
                site_value(s, v) / (1.0 + f64::from(distance[v as usize])),
                v,
                distance[v as usize],
            )
        })
        .collect();
    sites.sort_by(|a, b| b.0.total_cmp(&a.0));
    sites.truncate(3);
    sites
}
/// Threat of every other seat from `p`'s perspective (own entry is zero). With
/// `predicted_space`, the `space` component keys on the other seat's predicted next build
/// instead of scanning every site of `p`'s frontier they can reach: only a seat whose
/// predicted line ends on one of `p`'s best three sites counts, weighted by that site's
/// share of the frontier.
pub fn threats(s: &State, p: u8, predicted_space: bool) -> [Threat; 4] {
    let mut out = [Threat::default(); 4];
    let own_points = f64::from(public_points(s, p));
    let to_win = f64::from(s.settings.points_to_win.max(3));
    let own_distance = distances(s, p);
    let own_sites = frontier(s, p, &own_distance);
    let own_total: f64 = own_sites.iter().map(|x| x.0).sum();
    let own_length = f64::from(road_length(s, p));
    let own_army = f64::from(s.players[p as usize].played_knights);
    for q in 0..s.settings.players {
        if q == p {
            continue;
        }
        let points = f64::from(public_points(s, q));
        let race = ((points - own_points) * 0.15 + points / to_win * 0.7 - 0.15).clamp(0.0, 1.0);
        let mut contested = 0.0;
        if predicted_space {
            let build = super::predict::next_build(s, q, s.players[q as usize].resources);
            let takes_site = matches!(
                build,
                Some(super::predict::Build {
                    action: Action::BuildSettlement { .. } | Action::BuildRoad { .. },
                    ..
                })
            );
            if let (Some(b), true) = (build, takes_site)
                && let Some(v) = b.site
                && s.board.building(v) == 0
                && let Some(&(value, _, _)) = own_sites.iter().find(|&(_, w, _)| *w == v)
            {
                contested += value;
            }
        } else {
            let their_distance = distances(s, q);
            for &(value, v, d) in &own_sites {
                let theirs = their_distance[v as usize];
                if theirs <= 3 && theirs <= d + 1 {
                    contested += value;
                }
            }
        }
        let space = if own_total > 0.0 {
            (contested / own_total).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let their_length = f64::from(road_length(s, q));
        let their_army = f64::from(s.players[q as usize].played_knights);
        let mut awards = 0.0;
        let road_race =
            (own_length - their_length).abs() <= 1.5 && own_length.max(their_length) >= 4.0;
        if road_race && (s.longest_road == Some(q) || s.longest_road == Some(p)) {
            awards += 0.5;
        }
        let army_race = (own_army - their_army).abs() <= 1.0 && own_army.max(their_army) >= 2.0;
        if army_race && (s.largest_army == Some(q) || s.largest_army == Some(p)) {
            awards += 0.5;
        }
        let total = (0.65 * race + 0.45 * space + 0.25 * awards).clamp(0.0, 1.0);
        out[q as usize] = Threat {
            race,
            space,
            awards,
            total,
        };
    }
    out
}
/// The most threatening other seat, when any threat is material.
pub fn top_threat(threats: &[Threat; 4], players: u8) -> Option<(u8, Threat)> {
    (0..players)
        .map(|q| (q, threats[q as usize]))
        .filter(|(_, t)| t.total >= 0.3)
        .max_by(|a, b| a.1.total.total_cmp(&b.1.total))
}
/// The best site the top threat is heading for, if this seat could take or block it: the
/// vertex, and whether `p` can reach it within three roads.
pub fn contested_site(s: &State, p: u8, q: u8) -> Option<u8> {
    let theirs = distances(s, q);
    let ours = distances(s, p);
    frontier(s, q, &theirs)
        .into_iter()
        .find(|&(_, v, d)| ours[v as usize] <= 3 && ours[v as usize] <= d + 1)
        .map(|(_, v, _)| v)
}

/// What the partner gets out of a swap, in heuristic points: net cards, a build the swap
/// completes for them, and progress toward one. Reads their hand in a sampled world.
pub fn partner_gain(
    s: &State,
    q: u8,
    they_get: Resources,
    they_pay: Resources,
    card_value: f64,
    point: f64,
) -> f64 {
    let hand = s.players[q as usize].resources;
    let after = Resources(std::array::from_fn(|r| {
        (hand.0[r] + they_get.0[r]).saturating_sub(they_pay.0[r])
    }));
    let cards = card_value * (f64::from(they_get.total()) - f64::from(they_pay.total()));
    let mut unlock = 0.0f64;
    let mut progress = 0.0f64;
    for (cost, bonus) in [
        (Resources::CITY, 0.45),
        (Resources::SETTLEMENT, 0.4),
        (Resources::DEVELOPMENT, 0.15),
    ] {
        let before = deficit(hand, cost);
        let later = deficit(after, cost);
        if before > 0.0 && later <= 0.0 {
            unlock = unlock.max(bonus * point);
        }
        progress = progress.max(before - later);
    }
    cards + unlock + 0.3 * card_value * progress
}
/// A build the swap completes for the partner under their minimum known hand: what a
/// declining seat can point to without seeing their cards.
pub fn public_unlock(
    k: &ResourceKnowledge,
    q: u8,
    they_get: Resources,
    they_pay: Resources,
) -> Option<&'static str> {
    let minimum = Resources(k.players[q as usize].minimum);
    let after = Resources(std::array::from_fn(|r| {
        (minimum.0[r] + they_get.0[r]).saturating_sub(they_pay.0[r])
    }));
    for (cost, name) in [
        (Resources::CITY, "city"),
        (Resources::SETTLEMENT, "settlement"),
    ] {
        if deficit(minimum, cost) > 0.0 && deficit(after, cost) <= 0.0 {
            return Some(name);
        }
    }
    None
}

/// What a candidate command is, for valuation and table talk.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A proposal to `partner`; `ask` is true when the seat asks for more cards than it gives.
    Offer {
        partner: u8,
        ask: bool,
    },
    Accept {
        offer: TradeOffer,
    },
    /// A reply to `offer` with different terms; `acceptance` is the estimated chance the
    /// offerer takes it, from their revealed interest.
    Counter {
        offer: TradeOffer,
        acceptance: f64,
    },
    Reject {
        offer: TradeOffer,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub command: Command,
    pub action: Action,
    pub kind: Kind,
}
/// Offers this seat has made since the current turn began, from the public record.
pub fn offers_this_turn(history: &[EventBatch], seat: u8) -> Vec<TradeOffer> {
    let mut out = Vec::new();
    for batch in history {
        for event in &batch.events {
            match event {
                PublicEvent::Rule {
                    event: Event::TurnChanged { .. },
                }
                | PublicEvent::Started => out.clear(),
                PublicEvent::TradeOffered { offer } if offer.from == seat => {
                    out.push(offer.clone());
                }
                _ => {}
            }
        }
    }
    out
}
fn wanted_resources(own: Resources) -> Vec<(Resource, u8)> {
    // Resources short for any build, with the largest shortfall first.
    let mut out: Vec<(Resource, u8)> = Vec::new();
    for cost in [
        Resources::CITY,
        Resources::SETTLEMENT,
        Resources::ROAD,
        Resources::DEVELOPMENT,
    ] {
        for r in Resource::ALL {
            let short = cost.0[r as usize].saturating_sub(own.0[r as usize]);
            if short > 0 {
                match out.iter_mut().find(|(x, _)| *x == r) {
                    Some(entry) => entry.1 = entry.1.max(short),
                    None => out.push((r, short)),
                }
            }
        }
    }
    out.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    out
}
/// Candidate proposals for the current player: swaps toward a build, sized one or two for
/// one, and asks of two for one when the partner is known to hold plenty. Partners whose
/// threat is at or above `avoid` are not approached.
#[allow(clippy::too_many_arguments)]
pub fn proposals(
    view: &GameView,
    k: &ResourceKnowledge,
    threats: &[Threat; 4],
    avoid: f64,
    history: &[EventBatch],
    offered_turn: Option<u32>,
) -> Vec<Decision> {
    let Some(o) = &view.game else {
        return vec![];
    };
    let (Some(p), Some(own)) = (o.seat, o.own) else {
        return vec![];
    };
    if o.phase != Phase::Main
        || o.current != p
        || offered_turn == Some(o.turn)
        || !offers_this_turn(history, p).is_empty()
        || view
            .offers
            .iter()
            .any(|offer| offer.from == p && offer.status == OfferStatus::Open)
    {
        return vec![];
    }
    let mut out: Vec<Decision> = Vec::new();
    let mut push = |partner: u8, give: Resources, receive: Resources, ask: bool| {
        let action = Action::Trade {
            other: partner,
            give,
            receive,
        };
        if out.iter().any(|d| d.action == action) {
            return;
        }
        out.push(Decision {
            command: Command::OfferTrade {
                to: vec![partner],
                give,
                receive,
                message: None,
            },
            action,
            kind: Kind::Offer { partner, ask },
        });
    };
    let wanted = wanted_resources(own.resources);
    for other in 0..o.settings.players {
        if other == p || threats[other as usize].total >= avoid {
            continue;
        }
        let estimate = &k.players[other as usize];
        if estimate.total == 0 {
            continue;
        }
        for &(want, _) in wanted.iter().take(3) {
            let r = want as usize;
            if !estimate.can_have(Resources::one(want, 1)) {
                continue;
            }
            for surplus in Resource::ALL {
                if surplus == want {
                    continue;
                }
                let s = surplus as usize;
                // Keep what every build needs: give only cards beyond the largest cost.
                let keep = [
                    Resources::CITY,
                    Resources::SETTLEMENT,
                    Resources::ROAD,
                    Resources::DEVELOPMENT,
                ]
                .iter()
                .filter(|cost| cost.0[r] > own.resources.0[r])
                .map(|cost| cost.0[s])
                .min()
                .unwrap_or(0);
                let spare = own.resources.0[s].saturating_sub(keep);
                for n in 1..=2u8 {
                    if spare < n {
                        continue;
                    }
                    push(
                        other,
                        Resources::one(surplus, n),
                        Resources::one(want, 1),
                        false,
                    );
                }
                // Ask two for one when the partner is known to hold at least two.
                if spare >= 1 && estimate.minimum[r] >= 2 {
                    push(
                        other,
                        Resources::one(surplus, 1),
                        Resources::one(want, 2),
                        true,
                    );
                }
            }
        }
    }
    out
}
fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(24)
        .collect::<String>()
        .trim()
        .to_string()
}
/// A seat's display name from the view, or a neutral fallback.
pub fn name(view: &GameView, seat: u8) -> String {
    view.members
        .iter()
        .find(|m| m.seat == Some(seat))
        .map(|m| sanitize(&m.name))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("seat {}", seat + 1))
}
const NAMES: [&str; 5] = ["brick", "lumber", "wool", "grain", "ore"];
/// "2 wool and 1 ore".
pub fn bundle(r: Resources) -> String {
    let parts: Vec<String> = (0..5)
        .filter(|&i| r.0[i] > 0)
        .map(|i| format!("{} {}", r.0[i], NAMES[i]))
        .collect();
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(" and ")
    }
}
fn build_name(a: &Action) -> Option<&'static str> {
    match a {
        Action::BuildCity { .. } => Some("a city"),
        Action::BuildSettlement { .. } => Some("a settlement"),
        Action::BuildRoad { .. } => Some("a road"),
        Action::BuyDevelopment => Some("a development card"),
        _ => None,
    }
}
/// Table talk for a proposal. `unlocked` are the builds the swap makes affordable; `block`
/// names the seat whose expansion the first unlocked build would deny.
pub fn offer_text(
    view: &GameView,
    to: &[u8],
    give: Resources,
    receive: Resources,
    unlocked: &[Action],
    block: Option<u8>,
    ask: bool,
) -> String {
    let who = if to.len() == 1 {
        name(view, to[0])
    } else {
        "Anyone".to_string()
    };
    let mut text = format!("{who}: {} for {}?", bundle(give), bundle(receive));
    if let Some(q) = block {
        text += &format!(
            " I want to build before {} closes off that corner.",
            name(view, q)
        );
    } else if let Some(build) = unlocked.iter().find_map(build_name) {
        text += &format!(" That gets me {build}.");
    } else if ask {
        text += " I have spare cards and you seem well stocked.";
    } else {
        text += " Spare cards on my side, and I could use those.";
    }
    text
}
/// Table talk for accepting `offer` from this seat's perspective.
pub fn accept_text(view: &GameView, offer: &TradeOffer) -> String {
    format!(
        "Deal, {}. {} for {} works for me.",
        name(view, offer.from),
        bundle(offer.receive),
        bundle(offer.give)
    )
}
/// Table talk for a counter: the seat gives `give` and wants `receive`.
pub fn counter_text(
    view: &GameView,
    offer: &TradeOffer,
    give: Resources,
    receive: Resources,
) -> String {
    let who = name(view, offer.from);
    if receive.total() > offer.give.total() {
        format!(
            "{who}, one for one is thin for me. {} for {} and we have a deal.",
            bundle(give),
            bundle(receive)
        )
    } else {
        format!(
            "{who}, I'd rather do {} for {}.",
            bundle(give),
            bundle(receive)
        )
    }
}
/// Why a seat declines an offer.
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// The offerer leads the race or is near the finish.
    Leader {
        points: u8,
    },
    /// The offerer competes for the same ground.
    Space,
    /// The swap would complete a named build for the offerer.
    Unlock {
        build: &'static str,
    },
    /// This seat keeps the requested cards for its own target.
    Need {
        resource: Resource,
        build: Option<&'static str>,
    },
    /// The terms are too thin: more of the offered resource would work.
    Thin,
    Plain,
}
/// Table talk for declining `offer`. Early in the game a short pass is enough; later the
/// reason is spelled out.
pub fn decline_text(
    view: &GameView,
    offer: &TradeOffer,
    reason: &Reason,
    turn: u32,
    players: u8,
) -> String {
    let who = name(view, offer.from);
    let early = turn <= u32::from(players) * 2;
    let asked = (0..5)
        .max_by_key(|&r| offer.receive.0[r])
        .map(|r| NAMES[r])
        .unwrap_or("that");
    match reason {
        Reason::Leader { points } => {
            format!("No, {who}. You're at {points} points and I'm not helping you close it out.")
        }
        Reason::Space => format!(
            "No {asked} from me, {who}. We're racing for the same ground and I'd rather build there first."
        ),
        Reason::Unlock { build } => {
            format!("I'll pass, {who}. That {asked} would hand you a {build}.")
        }
        Reason::Need { resource, build } => match build {
            Some(build) => format!(
                "Not this time, {who}. I'm keeping my {} for {build}.",
                NAMES[*resource as usize]
            ),
            None => format!(
                "Not this time, {who}. I need my {} myself.",
                NAMES[*resource as usize]
            ),
        },
        Reason::Thin => format!(
            "One for one doesn't do it, {who}. Add another {} and ask again.",
            bundle(offer.give).split(' ').nth(1).unwrap_or("card")
        ),
        Reason::Plain if early => format!("Not right now, {who}."),
        Reason::Plain => format!("I'll pass, {who}. That doesn't move my next build."),
    }
}
/// Candidate answers to the open offers addressed to this seat: accepting, and counters
/// when the offerer is the current player, this seat is not, and it has not answered yet.
pub fn responses(view: &GameView, k: &ResourceKnowledge, history: &[EventBatch]) -> Vec<Decision> {
    let Some(o) = &view.game else {
        return vec![];
    };
    let (Some(p), Some(own)) = (o.seat, o.own) else {
        return vec![];
    };
    if o.phase != Phase::Main {
        return vec![];
    }
    let mine = offers_this_turn(history, p);
    let my_open = view
        .offers
        .iter()
        .any(|offer| offer.from == p && offer.status == OfferStatus::Open);
    let mut out = Vec::new();
    for offer in &view.offers {
        if offer.status != OfferStatus::Open
            || !offer.to.contains(&p)
            || offer.from == p
            || offer.declined.contains(&p)
            || !(o.current == p || o.current == offer.from)
        {
            continue;
        }
        if own.resources.contains(offer.receive) {
            out.push(Decision {
                command: Command::AcceptTrade {
                    offer_id: offer.id.clone(),
                },
                action: Action::Trade {
                    other: offer.from,
                    give: offer.receive,
                    receive: offer.give,
                },
                kind: Kind::Accept {
                    offer: offer.clone(),
                },
            });
        }
        // Counters: only as the non-current seat, once per foreign offer, with nothing of
        // ours still open.
        let answered = mine.iter().any(|m| m.to == vec![offer.from]);
        if o.current != offer.from || o.current == p || my_open || answered {
            continue;
        }
        let eagerness = eagerness(k, offer);
        let estimate = &k.players[offer.from as usize];
        let mut counters: Vec<(Resources, Resources, f64)> = Vec::new();
        // More of what they offered.
        let main = (0..5).max_by_key(|&r| offer.give.0[r]).unwrap_or(0);
        let mut more = offer.give;
        more.0[main] += 1;
        if estimate.can_have(more) {
            counters.push((offer.receive, more, eagerness * 0.75));
        }
        // One extra card of something else we are short of.
        for (want, _) in wanted_resources(own.resources).into_iter().take(2) {
            let r = want as usize;
            if offer.give.0[r] > 0 || offer.receive.0[r] > 0 {
                continue;
            }
            let mut plus = offer.give;
            plus.0[r] += 1;
            if estimate.can_have(plus) {
                counters.push((offer.receive, plus, eagerness * 0.65));
            }
            // Or the same count of a different resource we need more.
            let swap = Resources::one(want, offer.give.total().clamp(1, 2) as u8);
            if estimate.can_have(swap) && swap.disjoint(offer.receive) {
                counters.push((offer.receive, swap, eagerness * 0.6));
            }
        }
        // Give less when they asked for two.
        if offer.receive.total() >= 2 {
            let top = (0..5).max_by_key(|&r| offer.receive.0[r]).unwrap_or(0);
            let mut less = offer.receive;
            less.0[top] -= 1;
            if less.total() > 0 {
                counters.push((less, offer.give, eagerness * 0.7));
            }
        }
        for (give, receive, acceptance) in counters.into_iter().take(4) {
            if give.total() == 0
                || receive.total() == 0
                || !give.disjoint(receive)
                || !own.resources.contains(give)
            {
                continue;
            }
            let action = Action::Trade {
                other: offer.from,
                give,
                receive,
            };
            if out.iter().any(|d: &Decision| d.action == action) {
                continue;
            }
            out.push(Decision {
                command: Command::OfferTrade {
                    to: vec![offer.from],
                    give,
                    receive,
                    message: Some(Communication {
                        intent: Intent::RequestTrade,
                        resources: Some(receive),
                        text: Some(counter_text(view, offer, give, receive)),
                    }),
                },
                action,
                kind: Kind::Counter {
                    offer: offer.clone(),
                    acceptance,
                },
            });
        }
    }
    out
}
/// How much the offerer wants the cards they asked for, from what is public: a swap that
/// completes a build for them under their minimum hand, a large hand, and an explicit
/// request all raise it.
pub fn eagerness(k: &ResourceKnowledge, offer: &TradeOffer) -> f64 {
    let mut e = 0.5f64;
    if public_unlock(k, offer.from, offer.receive, offer.give).is_some() {
        e += 0.25;
    }
    if k.players[offer.from as usize].total >= 6 {
        e += 0.1;
    }
    if offer
        .message
        .as_ref()
        .is_some_and(|m| m.intent == Intent::RequestTrade)
    {
        e += 0.05;
    }
    e.clamp(0.0, 0.95)
}
/// The command that declines `offer` explicitly.
pub fn reject(offer: &TradeOffer) -> Decision {
    Decision {
        command: Command::RejectTrade {
            offer_id: offer.id.clone(),
        },
        action: Action::EndTurn,
        kind: Kind::Reject {
            offer: offer.clone(),
        },
    }
}
