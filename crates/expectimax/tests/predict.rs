//! Build prediction: the next piece a seat will complete, the denial leaf term, the
//! prediction-guided opponent model, and the predicted `space` threat.
#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::{SearchConfig, Weights, bargain, leaf, predict};

fn main_position() -> State {
    let (mut s, _) = common::fixture(4);
    while !(s.phase == Phase::Main && s.current == 0) {
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

#[test]
fn prediction_names_the_site_the_line_ends_on() {
    let s = main_position();
    for q in 0..4u8 {
        let hand = s.players[q as usize].resources;
        if let Some(build) = predict::next_build(&s, q, hand) {
            match build.action {
                Action::BuildRoad { .. } => {
                    // The road is the first piece of the expansion line; the site is the
                    // expansion target and is still open.
                    let (vertex, _, d) =
                        settlers_expectimax::v2::opponent::expansion(&s, q).expect("target");
                    assert!(d > 0);
                    assert_eq!(build.site, Some(vertex));
                }
                Action::BuildSettlement { vertex } => assert_eq!(build.site, Some(vertex)),
                Action::BuildCity { vertex } => assert_eq!(build.site, Some(vertex)),
                Action::BuyDevelopment => assert_eq!(build.site, None),
                _ => panic!("unexpected predicted action {:?}", build.action),
            }
            assert!(build.eta >= 0.0);
            assert!(build.site_eta >= build.eta - 1e-9);
        } else {
            let own = s.players[q as usize];
            assert!(own.settlements >= 5 && own.cities >= 4);
        }
    }
}

#[test]
fn guided_opponent_completes_the_affordable_piece_and_saves_otherwise() {
    let (mut s, _) = common::fixture(4);
    for q in 0..4u8 {
        // Advance to q's own main phase: the guided model answers for the acting seat.
        while !(s.phase == Phase::Main && s.current == q) {
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
        // A hand that can pay for every piece of any line completes the predicted piece.
        let rich = Resources([4, 4, 3, 3, 3]);
        deal(&mut s, q, rich);
        if let Some(build) = predict::next_build(&s, q, rich) {
            assert_eq!(predict::act(&s, q, 0), build.action);
        }
        // With nothing in hand and no surplus to trade, the guided opponent saves.
        deal(&mut s, q, Resources::default());
        if let Some(build) = predict::next_build(&s, q, Resources::default()) {
            assert!(
                build.eta > 0.0,
                "empty hand cannot afford {:?}",
                build.action
            );
        }
        assert_eq!(predict::act(&s, q, 0), Action::EndTurn);
    }
}

#[test]
fn denial_term_discounts_open_sites_opponents_are_heading_for() {
    // Play the fixture forward with the builders and scan for contested ground: a seat whose
    // predicted line ends on an open site another seat could reach within three roads.
    let (mut s, _) = common::fixture(4);
    let mut contested = 0u32;
    let w = Weights {
        denial: 1.0,
        ..Weights::default()
    };
    'game: for _ in 0..5000 {
        if matches!(s.phase, Phase::Finished { .. }) {
            break;
        }
        if s.phase == Phase::Main {
            let plain = leaf::evaluate(&s, s.current, &Weights::default());
            assert!(
                !plain.0.contains_key("denial"),
                "the default leaf has no denial term"
            );
            let terms = leaf::evaluate(&s, s.current, &w);
            if let Some(denial) = terms.0.get("denial") {
                assert!(*denial <= 0.0);
                if *denial < 0.0 {
                    // The discount is exactly the sum over opponents heading for the seat's
                    // open frontier, weighted by the site's value and the time to afford it.
                    let p = s.current;
                    let ours = bargain::distances(&s, p);
                    let future = w.card_value * leaf::remaining_rolls(&s, &w);
                    let mut expected = 0.0f64;
                    for q in 0..4u8 {
                        if q == p {
                            continue;
                        }
                        if let Some(build) =
                            predict::next_build(&s, q, s.players[q as usize].resources)
                            && matches!(
                                build.action,
                                Action::BuildSettlement { .. } | Action::BuildRoad { .. }
                            )
                            && let Some(v) = build.site
                            && settlement_allowed(&s, p, v, true)
                            && ours[v as usize] <= 3
                        {
                            contested += 1;
                            expected += settlers_expectimax::leaf::site_value(&s, v) * future
                                / (1.0 + build.site_eta);
                        }
                    }
                    assert!(
                        (denial + expected).abs() < 1e-6,
                        "denial {denial} vs {expected}"
                    );
                    break 'game;
                }
            }
        }
        let p = match s.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => s.current,
        };
        let action = settlers_expectimax::v2::opponent::act(&s, p, 0);
        s = apply(&s, p, action).unwrap();
    }
    assert!(
        contested > 0,
        "no builder game from the fixture ever had contested ground"
    );
}

#[test]
fn predicted_space_keys_on_the_opponents_predicted_site() {
    let s = main_position();
    for p in 0..4u8 {
        let ours = bargain::distances(&s, p);
        let sites = bargain::frontier(&s, p, &ours);
        let total: f64 = sites.iter().map(|x| x.0).sum();
        let threats = bargain::threats(&s, p, true);
        for q in 0..4u8 {
            if q == p {
                assert_eq!(threats[q as usize].space, 0.0);
                continue;
            }
            let build = predict::next_build(&s, q, s.players[q as usize].resources);
            let takes_site = build.is_some_and(|b| {
                matches!(
                    b.action,
                    Action::BuildSettlement { .. } | Action::BuildRoad { .. }
                )
            });
            let in_frontier = build
                .and_then(|b| b.site)
                .is_some_and(|v| sites.iter().any(|&(_, w, _)| w == v));
            let space = threats[q as usize].space;
            if space > 0.0 {
                assert!(
                    takes_site && in_frontier,
                    "space {space} without a predicted frontier site"
                );
                let v = build.unwrap().site.unwrap();
                let value = sites
                    .iter()
                    .find(|&(_, w, _)| *w == v)
                    .map(|&(x, _, _)| x)
                    .unwrap();
                if total > 0.0 {
                    assert!((space - (value / total).clamp(0.0, 1.0)).abs() < 1e-9);
                }
            }
            assert!((0.0..=1.0).contains(&space));
        }
    }
}

#[test]
fn a_game_with_the_prediction_switches_plays_to_the_end() {
    let (mut state, mut history) = common::fixture(4);
    let config = SearchConfig {
        depth: 1,
        samples: 2,
        scenarios: 2,
        inner_scenarios: 1,
        node_budget: 2_000,
        predict: settlers_expectimax::v2::PredictConfig {
            opponents: true,
            threat: true,
        },
        weights: Weights {
            denial: 0.5,
            ..Weights::default()
        },
        ..SearchConfig::default()
    };
    let clock = || 0.0;
    for _ in 0..5000 {
        if matches!(state.phase, Phase::Finished { .. }) {
            break;
        }
        let actor = match state.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => state.current,
        };
        let action = if actor == 0 {
            let report = settlers_expectimax::v2::analyze_with(
                &common::view(&state, 0, history.len() as u64),
                &history,
                &config,
                None,
                &clock,
                &settlers_expectimax::v2::Leaf::Hand,
                &mut settlers_expectimax::v2::Cache::default(),
            )
            .unwrap_or_else(|e| panic!("turn {}, phase {:?}: {e}", state.turn, state.phase));
            match report.candidates[0].command.as_ref() {
                Some(settlers_protocol::Command::Act { action }) => *action,
                _ => Action::EndTurn,
            }
        } else {
            legal_actions(&state, actor)
                .into_iter()
                .find(|a| !matches!(a, Action::RelocateSettlement { .. }))
                .unwrap_or(Action::EndTurn)
        };
        let (next, events) = apply_recorded(&state, actor, action).unwrap();
        state = next;
        common::push(&mut history, events);
    }
    assert!(matches!(state.phase, Phase::Finished { .. }));
}
