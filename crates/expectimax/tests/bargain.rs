//! Bargaining: threat assessment, counters, reasoned declines, and the search's answers.
#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::bargain::{self, Reason};
use settlers_expectimax::v2::{SearchConfig, analyze};
use settlers_protocol::knowledge::ResourceKnowledge;
use settlers_protocol::{
    Command, Communication, GameView, Intent, MemberView, OfferStatus, TradeOffer,
};

fn quick() -> SearchConfig {
    SearchConfig {
        depth: 1,
        samples: 2,
        scenarios: 2,
        inner_scenarios: 1,
        ..SearchConfig::default()
    }
}
/// A main-phase position for seat 0 after setup, with `current` to move.
fn position(current: u8) -> State {
    let (mut s, _) = common::fixture(4);
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
        s = apply(&s, p, action).unwrap();
    }
    s
}
/// Put `hand` in seat `q`'s hand, taking the cards from the bank so the position stays
/// consistent with what the knowledge model can verify.
fn deal(s: &mut State, q: u8, hand: Resources) {
    let old = s.players[q as usize].resources;
    for r in 0..5 {
        s.bank.0[r] = (s.bank.0[r] + old.0[r]).saturating_sub(hand.0[r]);
    }
    s.players[q as usize].resources = hand;
}
fn named_view(s: &State, seat: u8, offers: Vec<TradeOffer>) -> GameView {
    let mut view = common::view(s, seat, 40);
    view.members = (0..4)
        .map(|i| MemberView {
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

#[test]
fn threats_follow_the_race_and_shared_ground() {
    let mut s = position(0);
    // Seat 1 is close to winning, seat 2 is far behind.
    s.players[1].settlements = 4;
    s.players[1].cities = 2;
    s.players[2].settlements = 1;
    s.players[2].cities = 0;
    let threats = bargain::threats(&s, 0, false);
    assert_eq!(threats[0].total, 0.0);
    assert!(threats[1].race > 0.6, "{:?}", threats[1]);
    assert!(threats[2].race < 0.2, "{:?}", threats[2]);
    assert!(threats[1].total > threats[2].total);
    assert!(
        (0..4).all(|q| (0.0..=1.0).contains(&threats[q].total)),
        "{threats:?}"
    );
    let top = bargain::top_threat(&threats, 4).map(|t| t.0);
    assert_eq!(top, Some(1));
}

#[test]
fn a_thin_offer_draws_a_counter_priced_by_public_eagerness() {
    let mut s = position(1);
    deal(&mut s, 0, Resources([0, 0, 0, 1, 2]));
    deal(&mut s, 1, Resources([0, 0, 3, 2, 0]));
    let offer = TradeOffer {
        id: "offer-1".into(),
        from: 1,
        to: vec![0],
        give: Resources::one(Resource::Wool, 1),
        receive: Resources::one(Resource::Ore, 1),
        message: Some(Communication {
            intent: Intent::RequestTrade,
            resources: Some(Resources::one(Resource::Ore, 1)),
            text: None,
        }),
        status: OfferStatus::Open,
        thinking: vec![],
        created_at_ms: None,
        expires_at_ms: None,
        declined: vec![],
    };
    let view = named_view(&s, 0, vec![offer.clone()]);
    let o = view.game.as_ref().unwrap();
    let knowledge = ResourceKnowledge::from_history(o, &[]);
    let answers = bargain::responses(&view, &knowledge, &[]);
    assert!(
        answers
            .iter()
            .any(|d| matches!(d.command, Command::AcceptTrade { .. }))
    );
    let counters: Vec<_> = answers
        .iter()
        .filter(|d| matches!(d.kind, bargain::Kind::Counter { .. }))
        .collect();
    assert!(
        !counters.is_empty(),
        "a one-for-one offer invites a counter"
    );
    let more = counters.iter().find(|d| {
        matches!(
            d.command,
            Command::OfferTrade { ref to, receive, give, .. }
                if *to == vec![1] && receive == Resources::one(Resource::Wool, 2) && give == Resources::one(Resource::Ore, 1)
        )
    });
    let more = more.expect("asks for two wool for the ore");
    let bargain::Kind::Counter { acceptance, .. } = &more.kind else {
        panic!("counter kind");
    };
    let eager = bargain::eagerness(&knowledge, &offer);
    assert!((0.3..=0.95).contains(&eager));
    assert!((acceptance - eager * 0.75).abs() < 1e-9);
    if let Command::OfferTrade { message, .. } = &more.command {
        let text = message.as_ref().unwrap().text.as_ref().unwrap();
        assert!(text.starts_with("Ben,"), "{text}");
        assert!(text.contains("2 wool"), "{text}");
    }
}

#[test]
fn decline_text_gives_reasons_later_in_the_game() {
    let s = position(1);
    let offer = TradeOffer {
        id: "offer-2".into(),
        from: 1,
        to: vec![0],
        give: Resources::one(Resource::Wool, 1),
        receive: Resources::one(Resource::Ore, 1),
        message: None,
        status: OfferStatus::Open,
        thinking: vec![],
        created_at_ms: None,
        expires_at_ms: None,
        declined: vec![],
    };
    let view = named_view(&s, 0, vec![offer.clone()]);
    let early = bargain::decline_text(&view, &offer, &Reason::Plain, 2, 4);
    assert_eq!(early, "Not right now, Ben.");
    let leader = bargain::decline_text(&view, &offer, &Reason::Leader { points: 8 }, 30, 4);
    assert!(leader.contains("8 points"), "{leader}");
    let space = bargain::decline_text(&view, &offer, &Reason::Space, 30, 4);
    assert!(
        space.contains("same ground") && space.contains("ore"),
        "{space}"
    );
    let unlock = bargain::decline_text(&view, &offer, &Reason::Unlock { build: "city" }, 30, 4);
    assert!(unlock.contains("city"), "{unlock}");
    let need = bargain::decline_text(
        &view,
        &offer,
        &Reason::Need {
            resource: Resource::Ore,
            build: Some("a city"),
        },
        30,
        4,
    );
    assert!(need.contains("keeping my ore"), "{need}");
}

#[test]
fn the_search_answers_an_open_offer_with_words() {
    let mut s = position(1);
    deal(&mut s, 0, Resources([1, 1, 0, 1, 2]));
    deal(&mut s, 1, Resources([0, 0, 3, 2, 0]));
    let offer = TradeOffer {
        id: "offer-3".into(),
        from: 1,
        to: vec![0],
        give: Resources::one(Resource::Wool, 1),
        receive: Resources::one(Resource::Ore, 1),
        message: None,
        status: OfferStatus::Open,
        thinking: vec![],
        created_at_ms: None,
        expires_at_ms: None,
        declined: vec![],
    };
    let view = named_view(&s, 0, vec![offer]);
    let report = analyze(&view, &[], &quick(), None, &|| 0.0).unwrap();
    let top = &report.candidates[0];
    let kinds: Vec<&str> = report
        .candidates
        .iter()
        .map(|c| match &c.command {
            Some(Command::AcceptTrade { .. }) => "accept",
            Some(Command::OfferTrade { .. }) => "counter",
            Some(Command::RejectTrade { .. }) => "reject",
            Some(_) => "other",
            None => "wait",
        })
        .collect();
    assert!(kinds.contains(&"accept"), "{kinds:?}");
    assert!(kinds.contains(&"counter"), "{kinds:?}");
    assert!(
        !kinds.contains(&"wait") || !kinds.contains(&"reject"),
        "{kinds:?}"
    );
    match &top.command {
        Some(Command::AcceptTrade { .. }) | Some(Command::RejectTrade { .. }) => {
            let speech = top.speech.as_ref().expect("table talk");
            assert!(speech.contains("Ben"), "{speech}");
        }
        Some(Command::OfferTrade { to, message, .. }) => {
            assert_eq!(to, &vec![1]);
            assert!(message.as_ref().unwrap().text.is_some());
        }
        other => panic!("unexpected answer {other:?}"),
    }
    for c in &report.candidates {
        assert!((c.contributions.total() - c.expected_value).abs() < 1e-6);
    }
}

#[test]
fn proposals_carry_a_message_and_avoid_the_leader() {
    let mut s = position(0);
    deal(&mut s, 0, Resources([3, 3, 0, 0, 0]));
    s.players[1].settlements = 4;
    s.players[1].cities = 2;
    for q in 1..4 {
        deal(&mut s, q, Resources([0, 0, 2, 2, 2]));
    }
    let view = named_view(&s, 0, vec![]);
    let report = analyze(&view, &[], &quick(), None, &|| 0.0).unwrap();
    let offers: Vec<_> = report
        .candidates
        .iter()
        .filter_map(|c| match &c.command {
            Some(Command::OfferTrade { to, message, .. }) => Some((to.clone(), message.clone())),
            _ => None,
        })
        .collect();
    assert!(!offers.is_empty(), "surplus brick and lumber invite offers");
    for (to, message) in &offers {
        assert!(!to.contains(&1), "no offers to the leader: {to:?}");
        let text = message.as_ref().unwrap().text.as_ref().unwrap();
        assert!(text.contains(" for "), "{text}");
    }
    assert!(
        offers.iter().any(|(to, _)| to.len() >= 2),
        "one open offer to the safe seats"
    );
}
