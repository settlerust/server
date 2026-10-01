//! One-change switches of the bargaining: threat pricing, counters, two-for-one asks, and
//! the stubborn partner that never accepts an ask.
#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::{SearchConfig, analyze};
use settlers_protocol::{Command, EventBatch, GameView, MemberView, OfferStatus, TradeOffer};

fn quick() -> SearchConfig {
    SearchConfig {
        depth: 1,
        samples: 2,
        scenarios: 2,
        inner_scenarios: 1,
        ..SearchConfig::default()
    }
}
/// A main-phase position with `current` to move, and the public history that reached it.
fn position(current: u8) -> (State, Vec<EventBatch>) {
    let (mut s, mut history) = common::fixture(4);
    while !(s.phase == Phase::Main && s.current == current) {
        let p = match s.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => s.current,
        };
        let action = match s.phase {
            Phase::Roll => Action::Roll,
            Phase::Main => Action::EndTurn,
            _ => legal_actions(&s, p)[0],
        };
        let (next, events) = apply_recorded(&s, p, action).unwrap();
        s = next;
        common::push(&mut history, events);
    }
    (s, history)
}
/// Put `hand` in seat `q`'s hand from the bank, and record the cards as produced so the
/// public knowledge knows the seat holds at least that much.
fn deal(s: &mut State, history: &mut Vec<EventBatch>, q: u8, hand: Resources) {
    let old = s.players[q as usize].resources;
    for r in 0..5 {
        s.bank.0[r] = (s.bank.0[r] + old.0[r]).saturating_sub(hand.0[r]);
    }
    s.players[q as usize].resources = hand;
    if old.total() > 0 {
        common::push(
            history,
            vec![Event::Discarded {
                player: q,
                count: old.total(),
            }],
        );
    }
    common::push(
        history,
        vec![Event::Produced {
            player: q,
            resources: hand,
        }],
    );
}
fn named_view(s: &State, history: &[EventBatch], seat: u8, offers: Vec<TradeOffer>) -> GameView {
    let mut view = common::view(s, seat, history.len() as u64);
    view.members = (0..4)
        .map(|i| MemberView {
            computer_policy: None,
            idle: false,
            disconnected_at_ms: None,
            resolution: None,
            seat: Some(i),
            name: ["Ada", "Ben", "Cy", "Dee"][i as usize].into(),
            connected: true,
            ready: true,
            is_you: i == seat,
        })
        .collect();
    view.offers = offers;
    view
}
fn offer(id: &str, from: u8, to: u8, give: Resources, receive: Resources) -> TradeOffer {
    TradeOffer {
        id: id.into(),
        from,
        to: vec![to],
        give,
        receive,
        message: None,
        status: OfferStatus::Open,
        thinking: vec![],
        created_at_ms: None,
        expires_at_ms: None,
        declined: vec![],
    }
}
/// The commands the search considers, each labelled by kind.
fn kinds(view: &GameView, history: &[EventBatch], config: &SearchConfig) -> Vec<(String, Command)> {
    let report = analyze(view, history, config, None, &|| 0.0).unwrap();
    report
        .candidates
        .into_iter()
        .filter_map(|c| c.command)
        .map(|command| {
            let label = match &command {
                Command::AcceptTrade { .. } => "accept",
                Command::OfferTrade { .. } => "offer",
                Command::RejectTrade { .. } => "reject",
                _ => "act",
            };
            (label.to_string(), command)
        })
        .collect()
}

#[test]
fn threat_pricing_off_drops_the_premium_and_the_partner_cost() {
    let (mut s, mut history) = position(1);
    // Seat 1 leads the race, holds the makings of a city short one ore, and asks for it.
    s.players[1].settlements = 4;
    s.players[1].cities = 2;
    deal(&mut s, &mut history, 0, Resources([1, 1, 0, 1, 2]));
    deal(&mut s, &mut history, 1, Resources([0, 0, 3, 2, 2]));
    let ask = offer(
        "offer-1",
        1,
        0,
        Resources::one(Resource::Wool, 1),
        Resources::one(Resource::Ore, 1),
    );
    let view = named_view(&s, &history, 0, vec![ask]);
    let accept_terms = |config: &SearchConfig| {
        let report = analyze(&view, &history, config, None, &|| 0.0).unwrap();
        report
            .candidates
            .into_iter()
            .find(|c| matches!(c.command, Some(Command::AcceptTrade { .. })))
            .map(|c| c.contributions.0)
            .expect("seat 0 can afford the swap, so accepting is a candidate")
    };
    let priced = accept_terms(&quick());
    let plain = accept_terms(&SearchConfig {
        threat_pricing: false,
        ..quick()
    });
    let margin = quick().inclinations.trade_margin();
    assert!(
        priced["partner_threat"] < -1e-6,
        "the leader's gain is charged: {priced:?}"
    );
    assert!(
        priced["trade_margin"] < -margin - 1e-6,
        "the leader pays a margin premium: {priced:?}"
    );
    assert!(
        plain["partner_threat"].abs() < 1e-9,
        "no partner cost without threat pricing: {plain:?}"
    );
    assert!(
        (plain["trade_margin"] + margin).abs() < 1e-9,
        "the plain margin without threat pricing: {plain:?}"
    );
}

#[test]
fn counters_off_leaves_only_accepting_or_declining() {
    let (mut s, mut history) = position(1);
    deal(&mut s, &mut history, 0, Resources([0, 0, 0, 1, 2]));
    deal(&mut s, &mut history, 1, Resources([0, 0, 3, 2, 0]));
    let thin = offer(
        "offer-2",
        1,
        0,
        Resources::one(Resource::Wool, 1),
        Resources::one(Resource::Ore, 1),
    );
    let view = named_view(&s, &history, 0, vec![thin]);
    let with = kinds(&view, &history, &quick());
    assert!(
        with.iter().any(|(k, _)| k == "offer"),
        "a thin offer draws a counter by default: {with:?}"
    );
    let without = kinds(
        &view,
        &history,
        &SearchConfig {
            counters: false,
            ..quick()
        },
    );
    assert!(
        without.iter().all(|(k, _)| k != "offer"),
        "no counters when the switch is off: {without:?}"
    );
    assert!(
        without.iter().any(|(k, _)| k == "accept" || k == "reject"),
        "the offer is still answered: {without:?}"
    );
}

#[test]
fn asks_off_never_proposes_two_for_one() {
    let (mut s, mut history) = position(0);
    deal(&mut s, &mut history, 0, Resources([3, 3, 0, 0, 0]));
    for q in 1..4 {
        deal(&mut s, &mut history, q, Resources([0, 0, 2, 2, 3]));
    }
    let view = named_view(&s, &history, 0, vec![]);
    let asks = |config: &SearchConfig| -> Vec<(Resources, Resources)> {
        kinds(&view, &history, config)
            .into_iter()
            .filter_map(|(_, command)| match command {
                Command::OfferTrade { give, receive, .. } if receive.total() > give.total() => {
                    Some((give, receive))
                }
                _ => None,
            })
            .collect()
    };
    let with = asks(&quick());
    assert!(
        !with.is_empty(),
        "partners known to hold plenty are asked two for one by default"
    );
    let swaps = kinds(&view, &history, &quick())
        .into_iter()
        .filter(|(k, _)| k == "offer")
        .count();
    assert!(swaps > with.len(), "swaps are proposed alongside the asks");
    let without = asks(&SearchConfig {
        asks: false,
        ..quick()
    });
    assert!(
        without.is_empty(),
        "no asks with the switch off: {without:?}"
    );
    let still = kinds(
        &view,
        &history,
        &SearchConfig {
            asks: false,
            ..quick()
        },
    )
    .into_iter()
    .filter(|(k, _)| k == "offer")
    .count();
    assert!(still > 0, "one-for-one swaps are still proposed");
    let control = asks(&SearchConfig {
        bargain: false,
        ..quick()
    });
    assert!(control.is_empty(), "bargain: false turns asks off too");
}

#[test]
fn a_stubborn_seat_refuses_every_ask_but_still_takes_even_terms() {
    let (mut s, mut history) = position(1);
    deal(&mut s, &mut history, 0, Resources([1, 1, 0, 1, 3]));
    deal(&mut s, &mut history, 1, Resources([0, 0, 3, 2, 0]));
    let ask = offer(
        "offer-3",
        1,
        0,
        Resources::one(Resource::Wool, 1),
        Resources::one(Resource::Ore, 2),
    );
    let even = offer(
        "offer-4",
        1,
        0,
        Resources::one(Resource::Wool, 1),
        Resources::one(Resource::Ore, 1),
    );
    let view = named_view(&s, &history, 0, vec![ask, even]);
    let accepts = |config: &SearchConfig| -> Vec<String> {
        kinds(&view, &history, config)
            .into_iter()
            .filter_map(|(_, command)| match command {
                Command::AcceptTrade { offer_id } => Some(offer_id),
                _ => None,
            })
            .collect()
    };
    let with = accepts(&quick());
    assert!(
        with.iter().any(|id| id == "offer-3") && with.iter().any(|id| id == "offer-4"),
        "both offers can be accepted by default: {with:?}"
    );
    let stubborn = accepts(&SearchConfig {
        accept_asks: false,
        ..quick()
    });
    assert!(
        !stubborn.iter().any(|id| id == "offer-3"),
        "the ask is never accepted: {stubborn:?}"
    );
    assert!(
        stubborn.iter().any(|id| id == "offer-4"),
        "even terms are still a candidate: {stubborn:?}"
    );
}

#[test]
fn the_switches_default_on_and_round_trip_through_json() {
    let config = SearchConfig::default();
    assert!(config.bargain && config.threat_pricing && config.counters && config.asks);
    assert!(config.accept_asks);
    let parsed: SearchConfig =
        serde_json::from_str(r#"{"depth":2,"accept_asks":false,"asks":false}"#).unwrap();
    assert!(!parsed.accept_asks && !parsed.asks && parsed.counters && parsed.threat_pricing);
    let text = serde_json::to_string(&parsed).unwrap();
    assert!(text.contains("\"accept_asks\":false"), "{text}");
}
