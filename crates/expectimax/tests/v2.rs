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
fn spend_down_adds_shedding_trades_only_over_the_limit() {
    let (mut state, history) = common::fixture(4);
    let ratio = trade_ratio(&state, 0, Resource::Brick);
    assert_eq!(ratio, 4, "fixture seat 0 holds no brick port");
    state.players[0].resources = Resources([4, 0, 4, 4, 0]);
    state.bank = Resources(std::array::from_fn(|r| {
        19 - state.players.iter().map(|p| p.resources.0[r]).sum::<u8>()
    }));
    state.phase = Phase::Main;
    state.current = 0;
    let trades = |config: &SearchConfig| {
        let view = common::view(&state, 0, history.len() as u64);
        let report = analyze(&view, &history, config, None, &|| 0.0).unwrap();
        report
            .candidates
            .iter()
            .filter_map(|c| match &c.command {
                Some(Command::Act {
                    action: Action::BankTrade { give, receive },
                }) => Some((*give, *receive)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let plain = trades(&quick());
    let switched = trades(&SearchConfig {
        spend_down: true,
        ..quick()
    });
    // Every switched-only trade sheds cards without reducing a build deficit.
    let added: Vec<_> = switched.iter().filter(|t| !plain.contains(t)).collect();
    assert!(!added.is_empty(), "spend_down added no trades");
    for (give, receive) in added {
        assert!(give.total() > receive.total(), "not a shedding trade");
    }
    // Giving four brick for one grain reduces no build deficit, so the control never lists it.
    let shedding = (
        Resources::one(Resource::Brick, 4),
        Resources::one(Resource::Grain, 1),
    );
    assert!(!plain.contains(&shedding));
    assert!(switched.contains(&shedding));
    // Under the limit the switch adds nothing.
    state.players[0].resources = Resources([2, 0, 2, 2, 0]);
    state.bank = Resources(std::array::from_fn(|r| {
        19 - state.players.iter().map(|p| p.resources.0[r]).sum::<u8>()
    }));
    let view = common::view(&state, 0, history.len() as u64);
    let plain = analyze(&view, &history, &quick(), None, &|| 0.0).unwrap();
    let switched = analyze(
        &view,
        &history,
        &SearchConfig {
            spend_down: true,
            ..quick()
        },
        None,
        &|| 0.0,
    )
    .unwrap();
    assert_eq!(
        plain
            .candidates
            .iter()
            .map(|c| c.expected_value.to_bits())
            .collect::<Vec<_>>(),
        switched
            .candidates
            .iter()
            .map(|c| c.expected_value.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn discard_projection_values_the_kept_hand_after_expected_production() {
    use settlers_expectimax::v2::search::Search;
    let (mut state, _) = common::fixture(4);
    state.players[0].resources = Resources([2, 2, 2, 2, 8]);
    state.phase = Phase::Discard { pending: 1 };
    if state.layout.resource(state.robber) == Some(Resource::Ore) {
        state.robber = state.layout.desert();
    }
    // Give seat 0 cities on the best ore hexes, worth more than one ore per round.
    let mut ore: Vec<(u8, u8)> = (0..19u8)
        .filter(|&h| state.layout.resource(h) == Some(Resource::Ore))
        .map(|h| (6 - state.layout.number(h).abs_diff(7), h))
        .collect();
    ore.sort_by(|a, b| b.cmp(a));
    let mut buildings: Vec<u8> = (0..54).map(|v| state.board.building(v)).collect();
    let mut cities = 0;
    let mut rate = 0.0;
    for (pips, h) in ore {
        if rate > 1.0 {
            break;
        }
        let v = topology().hex_vertices[h as usize][0];
        buildings[v as usize] = 5; // a city of seat 0 (building value player + 5)
        rate += 2.0 * f64::from(pips) / 36.0 * (4.0 * 5.0 / 6.0);
        cities += 1;
    }
    assert!(cities >= 1 && rate > 1.0 && rate < 3.0, "ore rate {rate}");
    let roads: Vec<Option<u8>> = (0..72).map(|e| state.board.road(e)).collect();
    state.board = Board::from_public(&buildings, &roads).unwrap();
    // The default ranking is production-blind; projection changes the top discard.
    let plain = Search::discard_candidates(&state, 0, 4, false);
    let projected = Search::discard_candidates(&state, 0, 4, true);
    assert_ne!(plain[0], projected[0]);
    for a in plain.iter().chain(&projected) {
        let Action::Discard { resources } = a else {
            panic!("discard expected");
        };
        assert_eq!(resources.total(), 8);
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
            let mut cache = settlers_expectimax::v2::Cache::default();
            let report =
                analyze_with(&view, &history, &config, None, &|| 0.0, &leaf, &mut cache).unwrap();
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

#[test]
fn deepening_stops_on_settled_decisions() {
    // A roll-phase root with one candidate completes depth 1 only, whatever the cap.
    let (mut state, history) = common::fixture(4);
    let mut steps = 0;
    while !(state.phase == Phase::Roll
        && state.current == 0
        && state.players[0].development[0] == 0
        && state.turn > 8)
        && steps < 2000
    {
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let a = settlers_expectimax::v2::opponent::act(&state, actor, 0);
        state = apply(&state, actor, a).unwrap_or_else(|_| {
            apply(
                &state,
                actor,
                *legal_actions(&state, actor).first().unwrap(),
            )
            .unwrap()
        });
        steps += 1;
    }
    assert_eq!(state.phase, Phase::Roll);
    let view = common::view(&state, 0, history.len() as u64);
    let deep = SearchConfig {
        depth: 3,
        ..quick()
    };
    let report = analyze(&view, &history, &deep, None, &|| 0.0).unwrap();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.statistics.depth_completed, 1);
    // A wide gap rule settles a main-phase decision at depth 1; no rule deepens to the cap.
    let mut main = state;
    let mut steps = 0;
    while !(main.phase == Phase::Main && main.current == 0) && steps < 500 {
        let actor = match main.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => main.current,
        };
        let a = settlers_expectimax::v2::opponent::act(&main, actor, 0);
        main = apply(&main, actor, a).unwrap_or_else(|_| {
            apply(&main, actor, *legal_actions(&main, actor).first().unwrap()).unwrap()
        });
        steps += 1;
    }
    let view = common::view(&main, 0, history.len() as u64);
    let settled = analyze(
        &view,
        &history,
        &SearchConfig {
            depth: 3,
            deepen_gap: 1e-9,
            ..quick()
        },
        None,
        &|| 0.0,
    )
    .unwrap();
    let open = analyze(&view, &history, &deep, None, &|| 0.0).unwrap();
    assert!(settled.candidates.len() >= 2);
    assert_eq!(settled.statistics.depth_completed, 1);
    assert_eq!(open.statistics.depth_completed, 3);
}

#[test]
fn explain_mode_reports_expected_outcomes_with_probabilities() {
    let (mut state, history) = common::fixture(4);
    let mut steps = 0;
    while !(state.phase == Phase::Main && state.current == 0 && state.turn > 12) && steps < 3000 {
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let a = settlers_expectimax::v2::opponent::act(&state, actor, 0);
        state = apply(&state, actor, a).unwrap_or_else(|_| {
            apply(
                &state,
                actor,
                *legal_actions(&state, actor).first().unwrap(),
            )
            .unwrap()
        });
        steps += 1;
    }
    let view = common::view(&state, 0, history.len() as u64);
    let plain = analyze(&view, &history, &quick(), None, &|| 0.0).unwrap();
    let config = SearchConfig {
        explain: true,
        ..quick()
    };
    let report = analyze(&view, &history, &config, None, &|| 0.0).unwrap();
    // Explain mode never changes the decision or the values.
    assert_eq!(report.candidates[0].command, plain.candidates[0].command);
    assert!(
        (report.candidates[0].expected_value - plain.candidates[0].expected_value).abs() < 1e-9
    );
    assert!(plain.outcomes.is_none());
    let outcomes = report.outcomes.clone().expect("outcomes recorded");
    assert_eq!(outcomes.len(), report.statistics.depth_completed as usize);
    for level in &outcomes {
        let mass: f64 = level.groups.iter().map(|g| g.probability).sum::<f64>() + level.other;
        assert!(
            (mass - 1.0).abs() < 1e-6,
            "level {} mass {mass}",
            level.level
        );
        assert!(!level.groups.is_empty());
        for g in &level.groups {
            assert!(g.probability > 0.0 && g.probability <= 1.0 + 1e-9);
            assert!(g.value.is_finite());
            assert!(g.turn >= state.turn);
        }
        let mut sorted = level
            .groups
            .iter()
            .map(|g| g.probability)
            .collect::<Vec<_>>();
        sorted.sort_by(|a, b| b.total_cmp(a));
        assert_eq!(
            sorted,
            level
                .groups
                .iter()
                .map(|g| g.probability)
                .collect::<Vec<_>>()
        );
    }
    // The first level's mean value over outcomes is the chosen candidate's value at depth 1:
    // at deeper depths the outcome values carry the lookahead, so only finiteness is checked.
    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains("\"outcomes\""));
}

#[test]
fn information_set_key_hides_hidden_state_and_sees_public_state() {
    use settlers_expectimax::v2::key::information_set;
    let (state, _) = common::fixture(4);
    let base = information_set(&state, 0);
    assert_eq!(base, information_set(&state, 0));
    // Hidden: an opponent's hand composition at a fixed total, the deck composition at a fixed
    // count, and the random cursor.
    let mut hidden = state;
    let hand = &mut hidden.players[2].resources;
    if hand.total() > 0 {
        let from = (0..5).find(|&r| hand.0[r] > 0).unwrap();
        hand.0[from] -= 1;
        hand.0[(from + 1) % 5] += 1;
    }
    let total: u8 = hidden.deck.iter().sum();
    hidden.deck = [total, 0, 0, 0, 0];
    hidden.random.word += 99;
    assert_eq!(information_set(&hidden, 0), base);
    // Visible: own hand, an opponent's hand total, the robber, the turn.
    let mut own = state;
    own.players[0].resources.0[0] += 1;
    assert_ne!(information_set(&own, 0), base);
    let mut total_changed = state;
    total_changed.players[2].resources.0[0] += 1;
    assert_ne!(information_set(&total_changed, 0), base);
    let mut robber = state;
    robber.robber = (robber.robber + 1) % 19;
    assert_ne!(information_set(&robber, 0), base);
    let mut turn = state;
    turn.turn += 1;
    assert_ne!(information_set(&turn, 0), base);
    // The two words are independent: a changed position changes both.
    assert_ne!(information_set(&own, 0).1, base.1);
}

#[test]
fn shared_tables_within_a_decision_reproduce_fresh_tables_bit_for_bit() {
    use settlers_expectimax::v2::{Cache, CacheMode};
    let (mut state, history) = common::fixture(4);
    let fresh = SearchConfig {
        depth: 3,
        node_budget: 50_000_000,
        ..quick()
    };
    let shared = SearchConfig {
        cache: CacheMode::Decision,
        ..fresh.clone()
    };
    let mut cache = Cache::default();
    let mut compared = 0;
    let mut saved = 0u64;
    let mut steps = 0;
    while !matches!(state.phase, Phase::Finished { .. }) && steps < 1200 && compared < 40 {
        steps += 1;
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let action = if actor == 0 {
            let view = common::view(&state, actor, history.len() as u64);
            let a = analyze(&view, &history, &fresh, None, &|| 0.0).unwrap();
            let b = settlers_expectimax::v2::analyze_with(
                &view,
                &history,
                &shared,
                None,
                &|| 0.0,
                &settlers_expectimax::v2::Leaf::Hand,
                &mut cache,
            )
            .unwrap();
            assert_eq!(a.candidates.len(), b.candidates.len());
            for (x, y) in a.candidates.iter().zip(&b.candidates) {
                assert_eq!(x.command, y.command);
                assert_eq!(
                    x.expected_value.to_bits(),
                    y.expected_value.to_bits(),
                    "{:?}",
                    x.command
                );
            }
            assert_eq!(a.statistics.depth_completed, b.statistics.depth_completed);
            assert!(b.statistics.transitions <= a.statistics.transitions);
            saved += b.statistics.cached_transitions;
            compared += 1;
            match act(&state, actor, &a.candidates[0].command.clone().unwrap()) {
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
    assert!(compared >= 10, "{compared} decisions compared");
    assert!(saved > 0, "shared tables never saved work");
}

#[test]
fn total_budget_counts_every_pass_and_applies_the_deepening_share() {
    let (mut state, history) = common::fixture(4);
    let mut steps = 0;
    while !(state.phase == Phase::Main && state.current == 0 && state.turn > 8) && steps < 2000 {
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let a = settlers_expectimax::v2::opponent::act(&state, actor, 0);
        state = apply(&state, actor, a).unwrap_or_else(|_| {
            apply(
                &state,
                actor,
                *legal_actions(&state, actor).first().unwrap(),
            )
            .unwrap()
        });
        steps += 1;
    }
    let view = common::view(&state, 0, history.len() as u64);
    let cost = |depth: u8| {
        let single = SearchConfig {
            depth,
            iterative: false,
            node_budget: 50_000_000,
            ..quick()
        };
        analyze(&view, &history, &single, None, &|| 0.0)
            .unwrap()
            .statistics
            .transitions
    };
    let (t1, t2, t3) = (cost(1), cost(2), cost(3));
    assert!(t3 > t2 && t2 > t1 && t1 > 0);
    let per_pass = SearchConfig {
        depth: 3,
        node_budget: 50_000_000,
        ..quick()
    };
    let open = analyze(&view, &history, &per_pass, None, &|| 0.0).unwrap();
    assert!(open.candidates.len() >= 2);
    assert_eq!(open.statistics.depth_completed, 3);
    assert_eq!(open.statistics.transitions, t1 + t2 + t3);
    // A total budget that never binds reproduces the per-pass search bit for bit.
    let total = SearchConfig {
        total_budget: true,
        deepen_fraction: 1.0,
        ..per_pass
    };
    let same = analyze(&view, &history, &total, None, &|| 0.0).unwrap();
    assert_eq!(same.statistics.depth_completed, 3);
    for (a, b) in open.candidates.iter().zip(&same.candidates) {
        assert_eq!(a.command, b.command);
        assert_eq!(a.expected_value.to_bits(), b.expected_value.to_bits());
    }
    // The third pass runs out once the first two have used their share of the total.
    let cut = analyze(
        &view,
        &history,
        &SearchConfig {
            node_budget: t1 + t2 + t3 / 2,
            ..total.clone()
        },
        None,
        &|| 0.0,
    )
    .unwrap();
    assert_eq!(cut.statistics.depth_completed, 2);
    assert!(cut.statistics.aborted);
    assert!(cut.statistics.transitions < t1 + t2 + t3);
    // The deepening share stops the search after the pass that crosses it.
    let budget = t1 + t2 + t3 + 1;
    let shared = analyze(
        &view,
        &history,
        &SearchConfig {
            node_budget: budget,
            deepen_fraction: (t1 as f64 + 1.0) / budget as f64,
            ..total
        },
        None,
        &|| 0.0,
    )
    .unwrap();
    assert_eq!(shared.statistics.depth_completed, 2);
    assert!(!shared.statistics.aborted);
    assert_eq!(shared.statistics.transitions, t1 + t2);
}
