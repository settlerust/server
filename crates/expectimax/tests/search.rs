mod common;
use settlers_engine::*;
use settlers_expectimax::{Config, ExpectimaxPlayer, analyze};
use settlers_protocol::{Command, EventBatch, PublicEvent, knowledge::ResourceKnowledge};

#[test]
fn every_leaf_crosses_next_turn_even_with_one_node_and_depth_one() {
    for players in 2..=4 {
        let (s, history) = common::fixture(players);
        for seat in [s.current, (s.current + 1) % players] {
            let mut view = common::view(&s, seat, history.len() as u64);
            if seat != s.current {
                // A pending discard during another seat's turn is a genuine off-turn choice.
                let o = view.game.as_mut().unwrap();
                o.phase = Phase::Discard { pending: 1 << seat };
                o.legal_actions = vec![Action::Discard {
                    resources: automatic_discard(o.own.unwrap().resources),
                }];
            }
            let report = analyze(
                &view,
                &history,
                Config {
                    depth: 1,
                    samples: 1,
                    node_budget: 1,
                    ..Config::default()
                },
                None,
            )
            .unwrap();
            assert!(!report.candidates.is_empty());
            assert!(report.statistics.minimum_leaf_turn.unwrap() >= report.horizon_turn);
            for candidate in report.candidates {
                assert!((candidate.contributions.total() - candidate.expected_value).abs() < 1e-9);
            }
        }
    }
}

#[test]
fn trades_are_precomputed_with_accept_and_reject_branches_and_impossible_requests_excluded() {
    let (mut s, _) = common::fixture(2);
    s.phase = Phase::Main;
    s.current = 0;
    s.turn = 3;
    for p in &mut s.players {
        p.resources = Resources::default();
    }
    s.players[0].resources = Resources([0, 2, 0, 2, 2]);
    s.players[1].resources = Resources([0, 0, 0, 0, 1]);
    s.bank = Resources([19, 17, 19, 17, 16]);
    let history = vec![EventBatch {
        sequence: 1,
        timestamp_ms: 0,
        events: vec![
            PublicEvent::Started,
            PublicEvent::Rule {
                event: Event::Produced {
                    player: 0,
                    resources: s.players[0].resources,
                },
            },
            PublicEvent::Rule {
                event: Event::Produced {
                    player: 1,
                    resources: s.players[1].resources,
                },
            },
        ],
    }];
    let report = analyze(
        &common::view(&s, 0, 1),
        &history,
        Config {
            samples: 1,
            node_budget: 16,
            ..Config::default()
        },
        None,
    )
    .unwrap();
    let trades: Vec<_> = report
        .candidates
        .iter()
        .filter(|c| c.trade.is_some())
        .collect();
    assert!(!trades.is_empty());
    for candidate in trades {
        let Some(Command::OfferTrade { receive, .. }) = candidate.command else {
            panic!("trade candidate");
        };
        assert_eq!(receive, Resources([0, 0, 0, 0, 1]));
        let t = candidate.trade.as_ref().unwrap();
        assert!(
            t.unlocked_actions
                .iter()
                .any(|a| matches!(a, Action::BuildCity { .. }))
        );
        assert!(t.acceptance_probability > 0.0 && t.acceptance_probability < 1.0);
        assert!(
            (t.partner_affordability * t.acceptance_when_affordable - t.acceptance_probability)
                .abs()
                < 1e-9
        );
        let expected = t.acceptance_probability * t.accepted_value.unwrap()
            + (1.0 - t.acceptance_probability) * t.rejected_value;
        assert!((expected - candidate.expected_value).abs() < 1e-8);
    }
}

#[test]
fn rust_and_wasm_interface_are_deterministic_and_do_not_consume_authoritative_randomness() {
    let (mut s, history) = common::fixture(2);
    let config = Config {
        samples: 1,
        node_budget: 8,
        ..Config::default()
    };
    let v = common::view(&s, s.current, history.len() as u64);
    let a = analyze(&v, &history, config, None).unwrap();
    s.random = Random::new([99; 32]);
    let b = analyze(
        &common::view(&s, s.current, history.len() as u64),
        &history,
        config,
        None,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(b).unwrap()
    );
    let mut player = ExpectimaxPlayer::new();
    for batch in history.iter().rev().chain(history.iter()) {
        player
            .on_events(&serde_json::to_string(batch).unwrap())
            .unwrap();
    }
    let json = player
        .analyze(&serde_json::json!({ "view": v, "config": config }).to_string())
        .unwrap();
    assert_eq!(json, serde_json::to_string(&a).unwrap());
}

#[test]
fn missing_history_keeps_public_inventory_but_widens_opponent_allocations() {
    let (s, history) = common::fixture(4);
    let o = observe(&s, Some(s.current));
    let k = ResourceKnowledge::from_history(&o, &history);
    assert!(k.history_complete);
    assert!(k.players.iter().all(|p| p.is_exact()));
    let unknown = ResourceKnowledge::from_history(&o, &[]);
    assert!(!unknown.history_complete);
    assert!(o.players.iter().any(|p| p.seat != s.current
        && p.resource_count > 0
        && !unknown.players[p.seat as usize].is_exact()));
    assert_eq!(unknown.bank, s.bank);
}

#[test]
fn research_spectator_projection_is_rejected() {
    let (s, history) = common::fixture(2);
    let mut view = common::view(&s, s.current, history.len() as u64);
    view.research_hands = Some(vec![]);
    assert!(analyze(&view, &history, Config::default(), None).is_err());
}

#[test]
fn an_empty_bank_can_reveal_an_unknown_opponents_entire_resource_hand() {
    let (mut state, _) = common::fixture(2);
    state.current = 0;
    state.phase = Phase::Main;
    state.turn = 3;
    state.players[0].resources = Resources::default();
    state.players[1].resources = Resources::one(Resource::Brick, 19);
    state.bank = Resources([0, 19, 19, 19, 19]);
    let report = analyze(
        &common::view(&state, 0, 1),
        &[],
        Config {
            samples: 4,
            node_budget: 1,
            ..Config::default()
        },
        None,
    )
    .unwrap();
    assert!(!report.candidates.is_empty());
    assert!(!report.knowledge.history_complete);
    assert_eq!(report.knowledge.players[1].minimum, [19, 0, 0, 0, 0]);
    assert!(report.knowledge.players[1].is_exact());
    assert!(report.statistics.minimum_leaf_turn.unwrap() >= report.horizon_turn);
}

#[test]
fn every_setup_phase_reaches_the_players_first_completed_normal_turn() {
    for players in 2..=4 {
        let mut state = State::new(
            Settings {
                players,
                ..Settings::default()
            },
            [41; 32],
        )
        .unwrap();
        let mut history = vec![EventBatch {
            sequence: 1,
            timestamp_ms: 0,
            events: vec![PublicEvent::Started],
        }];
        while state.turn == 0 {
            let expected_horizon = 2 + u32::from((state.current + players - state.first) % players);
            let report = analyze(
                &common::view(&state, state.current, history.len() as u64),
                &history,
                Config {
                    samples: 1,
                    depth: 1,
                    node_budget: 1,
                    ..Config::default()
                },
                None,
            )
            .unwrap();
            assert_eq!(report.horizon_turn, expected_horizon);
            assert!(report.statistics.minimum_leaf_turn.unwrap() >= expected_horizon);
            let Some(Command::Act { action }) = report.candidates[0].command else {
                panic!("setup action expected");
            };
            let (next, events) = apply_recorded(&state, state.current, action).unwrap();
            state = next;
            common::push(&mut history, events);
        }
    }
}

#[test]
fn fair_leaf_features_ignore_opponents_private_cards_and_future_randomness() {
    let (mut state, _) = common::fixture(4);
    let seat = state.current;
    let before = settlers_expectimax::leaf::evaluate(&state, seat);
    for other in 0..4 {
        if other == seat {
            continue;
        }
        state.players[other as usize].resources = Resources([10, 9, 8, 7, 6]);
        state.players[other as usize].development = [2, 2, 2, 2, 5];
    }
    state.random = Random::new([99; 32]);
    state.deck = [0; 5];
    let after = settlers_expectimax::leaf::evaluate(&state, seat);
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}

#[test]
fn exact_bank_counts_distinguish_scarce_trade_supply_from_partner_willingness() {
    let (mut state, _) = common::fixture(4);
    state.phase = Phase::Main;
    state.current = 0;
    state.turn = 3;
    for player in &mut state.players {
        player.resources = Resources::default();
    }
    state.players[0].resources = Resources([0, 2, 0, 2, 2]);
    state.players[1].resources = Resources([3, 0, 0, 0, 1]);
    state.players[2].resources = Resources([4, 0, 0, 0, 0]);
    state.bank = Resources([12, 17, 19, 17, 16]);
    let analyze_state = |state: &State| {
        analyze(
            &common::view(state, 0, 12),
            &[],
            Config {
                samples: 16,
                node_budget: 1,
                ..Config::default()
            },
            None,
        )
        .unwrap()
    };
    let scarce = analyze_state(&state);
    let ore_trades: Vec<_> = scarce.candidates.iter().filter(|candidate| matches!(&candidate.command,
        Some(Command::OfferTrade { give, receive, .. }) if *give == Resources::one(Resource::Lumber, 1) && *receive == Resources::one(Resource::Ore, 1)
    )).collect();
    assert_eq!(ore_trades.len(), 2);
    let affordability: f64 = ore_trades
        .iter()
        .map(|candidate| candidate.trade.as_ref().unwrap().partner_affordability)
        .sum();
    assert!(
        (affordability - 1.0).abs() < 1e-9,
        "exactly one ore circulates among the two possible partners"
    );
    for candidate in ore_trades {
        let trade = candidate.trade.as_ref().unwrap();
        assert!(trade.partner_affordability > 0.0 && trade.partner_affordability < 1.0);
        assert!(
            (trade.acceptance_probability
                - trade.partner_affordability * trade.acceptance_when_affordable)
                .abs()
                < 1e-9
        );
    }
    assert!(
        scarce
            .candidates
            .iter()
            .all(|candidate| !matches!(&candidate.command,
                Some(Command::OfferTrade { receive, .. }) if receive.0[Resource::Wool as usize] > 0
            )),
        "a full wool bank proves nobody can supply wool"
    );

    state.players[1].resources = Resources::one(Resource::Ore, 4);
    state.players[2].resources = Resources::one(Resource::Ore, 4);
    state.bank = Resources([19, 17, 19, 17, 9]);
    let abundant = analyze_state(&state);
    assert_eq!(
        abundant.knowledge.opponent_totals,
        Some(Resources::one(Resource::Ore, 8))
    );
    for candidate in abundant
        .candidates
        .iter()
        .filter(|candidate| candidate.trade.is_some())
    {
        let trade = candidate.trade.as_ref().unwrap();
        assert_eq!(trade.partner_affordability, 1.0);
        assert!(
            trade.acceptance_when_affordable < 1.0,
            "holding cards does not prove willingness to trade"
        );
    }
}
