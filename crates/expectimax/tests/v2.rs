mod common;
use settlers_engine::*;
use settlers_expectimax::v2::{Inclinations, SearchConfig, Weights, analyze};
use settlers_protocol::Command;

fn quick() -> SearchConfig {
    SearchConfig {
        depth: 2,
        samples: 2,
        scenarios: 3,
        inner_scenarios: 2,
        afterstates: 24,
        ..SearchConfig::default()
    }
}
fn act(state: &State, actor: u8, command: &Command) -> Option<Action> {
    match command {
        Command::Act { action } => Some(*action),
        Command::OfferTrade {
            to, give, receive, ..
        } => {
            let trade = Action::Trade {
                other: to[0],
                give: *give,
                receive: *receive,
            };
            apply(state, actor, trade).ok().map(|_| trade)
        }
        _ => None,
    }
}

#[test]
fn tunable_search_finishes_games_with_every_player_count() {
    for players in 2..=4 {
        let (mut state, mut history) = common::fixture(players);
        let mut offered = None;
        let mut steps = 0;
        while !matches!(state.phase, Phase::Finished { .. }) {
            steps += 1;
            assert!(steps < 6000, "{players} players did not finish");
            let actor = match state.phase {
                Phase::Discard { pending } => pending.trailing_zeros() as u8,
                _ => state.current,
            };
            let action = if actor == 0 {
                let view = common::view(&state, actor, history.len() as u64);
                let report = analyze(&view, &history, &quick(), offered, &|| 0.0).unwrap();
                assert_eq!(report.policy, "expectimax-v2");
                assert!(report.statistics.depth_completed >= 1);
                for c in &report.candidates {
                    assert!((c.contributions.total() - c.expected_value).abs() < 1e-6);
                }
                let command = report.candidates[0].command.clone().unwrap();
                if matches!(command, Command::OfferTrade { .. }) {
                    offered = Some(state.turn);
                }
                match act(&state, actor, &command) {
                    Some(a) => a,
                    None => continue,
                }
            } else {
                legal_actions(&state, actor)
                    .into_iter()
                    .find(|a| !matches!(a, Action::RelocateSettlement { .. }))
                    .unwrap()
            };
            let (next, events) = apply_recorded(&state, actor, action).unwrap();
            state = next;
            common::push(&mut history, events);
        }
    }
}

#[test]
fn reports_are_deterministic_without_a_clock() {
    let (state, history) = common::fixture(4);
    let view = common::view(&state, state.current, history.len() as u64);
    let a = analyze(&view, &history, &quick(), None, &|| 0.0).unwrap();
    let b = analyze(&view, &history, &quick(), None, &|| 0.0).unwrap();
    assert_eq!(
        serde_json::to_string(&a.candidates).unwrap(),
        serde_json::to_string(&b.candidates).unwrap()
    );
    assert_eq!(a.statistics.transitions, b.statistics.transitions);
}

#[test]
fn time_budget_stops_deepening_and_keeps_a_completed_depth() {
    let (state, history) = common::fixture(4);
    let view = common::view(&state, state.current, history.len() as u64);
    let config = SearchConfig {
        depth: 6,
        time_budget_ms: 3.0,
        ..SearchConfig::default()
    };
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64() * 1000.0;
    let report = analyze(&view, &history, &config, None, &clock).unwrap();
    assert!(report.statistics.depth_completed < 6);
    assert!(!report.candidates.is_empty());
    assert!(report.statistics.time_ms < 1000.0);
}

#[test]
fn inclinations_scale_weights_symmetrically() {
    let base = Weights::default();
    let neutral = Inclinations::default().apply(base);
    assert_eq!(neutral, base);
    let aggressive = Inclinations {
        aggression: 1.0,
        ..Inclinations::default()
    }
    .apply(base);
    assert!((aggressive.opponent_max_point - base.opponent_max_point * 1.5).abs() < 1e-9);
    let cautious = Inclinations {
        risk: 0.0,
        ..Inclinations::default()
    }
    .apply(base);
    assert!((cautious.hand_risk - base.hand_risk * 1.5).abs() < 1e-9);
}

#[test]
fn discard_choices_cover_more_than_the_automatic_suggestion() {
    let (mut state, history) = common::fixture(4);
    state.players[0].resources = Resources([3, 3, 2, 1, 1]);
    state.bank = Resources(std::array::from_fn(|r| {
        19 - state.players.iter().map(|p| p.resources.0[r]).sum::<u8>()
    }));
    state.phase = Phase::Discard { pending: 1 };
    let mut view = common::view(&state, 0, history.len() as u64);
    view.game.as_mut().unwrap().legal_actions = vec![Action::Discard {
        resources: automatic_discard(Resources([3, 3, 2, 1, 1])),
    }];
    let report = analyze(&view, &history, &quick(), None, &|| 0.0).unwrap();
    assert!(report.candidates.len() > 1);
    for c in &report.candidates {
        let Some(Command::Act {
            action: Action::Discard { resources },
        }) = &c.command
        else {
            panic!("discard expected");
        };
        assert_eq!(resources.total(), 5);
    }
}

#[test]
fn reciprocity_scales_willingness_by_observed_acceptances() {
    use settlers_protocol::{EventBatch, OfferStatus, PublicEvent, TradeCloseReason, TradeOffer};
    let offer = |id: &str, to: Vec<u8>| PublicEvent::TradeOffered {
        offer: TradeOffer {
            id: id.into(),
            from: 0,
            to,
            give: Resources::one(Resource::Brick, 1),
            receive: Resources::one(Resource::Ore, 1),
            message: None,
            status: OfferStatus::Open,
            declined: vec![],
        },
    };
    let closed =
        |id: &str, reason: TradeCloseReason, winner: Option<u8>| PublicEvent::TradeClosed {
            offer_id: id.into(),
            reason,
            winner,
        };
    let history = vec![EventBatch {
        sequence: 1,
        timestamp_ms: 0,
        events: vec![
            offer("a", vec![1, 2]),
            closed("a", TradeCloseReason::Accepted, Some(1)),
            offer("b", vec![1, 2]),
            closed("b", TradeCloseReason::Declined, None),
            offer("c", vec![2]),
            closed("c", TradeCloseReason::Cancelled, None),
        ],
    }];
    let factor = settlers_expectimax::v2::reciprocity(&history, 0);
    assert!((factor[1] - 2.0 * 2.0 / 4.0).abs() < 1e-9);
    assert!((factor[2] - 2.0 * 1.0 / 4.0).abs() < 1e-9);
    assert!((factor[3] - 1.0).abs() < 1e-9);
}

#[test]
fn learned_tables_seat_the_search_and_rank_true_states() {
    use settlers_expectimax::ntuple::{Layout, Model};
    use settlers_expectimax::v2::{Leaf, LeafConfig, analyze_with, rank_state};
    use std::sync::Arc;
    let (mut state, history) = common::fixture(4);
    let model = Arc::new(Model::new(
        Layout::parse("hex,vertex,portfolio").unwrap(),
        0.25,
        false,
    ));
    let leaf = Leaf::tables(model, 1000.0, 0.0);
    let config = SearchConfig {
        depth: 1,
        propose: false,
        ..quick()
    };
    let mut ranked_decisions = 0;
    let mut steps = 0;
    while !matches!(state.phase, Phase::Finished { .. }) && steps < 400 {
        steps += 1;
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let action = if actor == 0 && state.turn < 12 {
            let view = common::view(&state, actor, history.len() as u64);
            let report = analyze_with(&view, &history, &config, None, &|| 0.0, &leaf).unwrap();
            assert_eq!(report.policy, "expectimax-v2-tables");
            for c in &report.candidates {
                assert!((c.contributions.total() - c.expected_value).abs() < 1e-6);
                assert!(
                    c.contributions.0.contains_key("tables")
                        || c.contributions.0.contains_key("terminal")
                        || c.contributions.0.contains_key("expected")
                );
            }
            let ranked = rank_state(&state, actor, &config, &leaf, 1).unwrap();
            assert!(!ranked.is_empty());
            assert!(
                ranked
                    .iter()
                    .all(|(a, v)| v.is_finite() && apply(&state, actor, *a).is_ok())
            );
            ranked_decisions += 1;
            match act(
                &state,
                actor,
                &report.candidates[0].command.clone().unwrap(),
            ) {
                Some(a) => a,
                None => continue,
            }
        } else {
            settlers_expectimax::v2::opponent::act(&state, actor, 0)
        };
        state = match apply(&state, actor, action) {
            Ok(next) => next,
            Err(_) => apply(
                &state,
                actor,
                *legal_actions(&state, actor).first().unwrap(),
            )
            .unwrap(),
        };
    }
    assert!(ranked_decisions >= 3);
    // A missing table file is a search error, never a silent fallback to the hand leaf.
    let missing = SearchConfig {
        leaf: LeafConfig {
            tables: Some("/nonexistent/tables.bin".into()),
            ..LeafConfig::default()
        },
        ..SearchConfig::default()
    };
    assert!(Leaf::from_config(&missing).is_err());
    assert!(
        Leaf::from_config(&SearchConfig::default())
            .unwrap()
            .is_hand()
    );
}
