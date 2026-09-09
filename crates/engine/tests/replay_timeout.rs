mod common;
use common::*;
use settlers_engine::*;
#[test]
fn timeout_completes_setup_then_rolls_and_ends_turn() {
    let mut s = game();
    for _ in 0..8 {
        s = timeout(&s).unwrap().0;
        conservation(&s);
    }
    assert_eq!(s.turn, 1);
    let n = timeout(&s).unwrap().0;
    assert_eq!(n.current, 1);
    assert_eq!(n.turn, 2);
    conservation(&n);
}
#[test]
fn timeout_seven_resets_robber_to_desert_without_stealing() {
    let mut s = started();
    hand(&mut s, 0, Resources([8, 0, 0, 0, 0]));
    s.phase = Phase::Discard { pending: 1 };
    s.robber = (s.layout.desert() + 1) % 19;
    let (n, events) = timeout(&s).unwrap();
    assert_eq!(n.robber, n.layout.desert());
    assert_eq!(n.players[0].resources.total(), 4);
    assert_eq!(n.current, 1);
    assert!(!events.iter().any(|e| matches!(e, Event::Stole { .. })));
}
#[test]
fn hidden_information_is_absent_from_observers_and_opponents() {
    let mut s = main_phase();
    dev(&mut s, 0, Development::VictoryPoint);
    let mine = observe(&s, Some(0));
    let theirs = observe(&s, Some(1));
    assert_eq!(mine.own.unwrap().development[4], 1);
    assert_eq!(theirs.own.unwrap().development[4], 0);
    assert_eq!(theirs.players[0].points, 2);
    let json = serde_json::to_string(&observe(&s, None)).unwrap();
    assert!(!json.contains("seed"));
    assert!(!json.contains("random"));
    assert!(!json.contains("new_development"));
    assert!(observe(&s, None).legal_actions.is_empty());
}
#[test]
fn seeded_actions_replay_bit_for_bit_and_conserve_inventory() {
    for seed in 0..12 {
        let initial = State::new(Settings::default(), [seed; 32]).unwrap();
        let mut s = initial;
        let mut actions = vec![];
        let mut rng = Random::new([seed + 1; 32]);
        for _ in 0..300 {
            let p = if let Phase::Discard { pending } = s.phase {
                pending.trailing_zeros() as u8
            } else {
                s.current
            };
            let legal = legal_actions(&s, p);
            if legal.is_empty() {
                assert!(matches!(s.phase, Phase::Finished { .. }));
                break;
            }
            let a = legal[rng.below(legal.len() as u32) as usize];
            s = apply(&s, p, a).unwrap();
            conservation(&s);
            actions.push((p, a));
        }
        let mut replay = initial;
        for (p, a) in actions {
            replay = apply(&replay, p, a).unwrap();
        }
        assert_eq!(replay, s);
        assert_eq!(
            serde_json::from_str::<State>(&serde_json::to_string(&s).unwrap()).unwrap(),
            s
        );
    }
}
#[test]
fn malformed_locations_and_chances_return_errors_without_panics() {
    let s = main_phase();
    for p in [0, 1, 4, 255] {
        for a in [
            Action::BuildRoad { edge: 255 },
            Action::BuildCity { vertex: 255 },
            Action::BuildSettlement { vertex: 255 },
            Action::MoveRobber {
                hex: 255,
                victim: Some(255),
            },
        ] {
            assert!(apply(&s, p, a).is_err());
        }
    }
    let s = started();
    assert_eq!(
        apply_with_chance(
            &s,
            0,
            Action::Roll,
            Chance::Dice {
                first: 0,
                second: 7
            }
        ),
        Err(RuleError::InvalidChance)
    );
}

#[test]
fn exact_chance_branches_have_unit_probability() {
    let s = started();
    let branches = chance_outcomes(&s, 0, Action::Roll).unwrap();
    assert_eq!(branches.len(), 36);
    assert_eq!(branches.iter().map(|b| b.numerator).sum::<u32>(), 36);
    let mut s = main_phase();
    hand(&mut s, 0, Resources::DEVELOPMENT);
    let branches = chance_outcomes(&s, 0, Action::BuyDevelopment).unwrap();
    assert_eq!(branches.iter().map(|b| b.numerator).sum::<u32>(), 25);
    assert_eq!(branches[0].denominator, 25);
}

#[test]
fn observations_expose_public_bank_counts_without_raw_opponent_hands() {
    let mut a = main_phase();
    let mut b = a;
    hand(&mut a, 1, Resources([4, 0, 0, 0, 0]));
    hand(&mut b, 1, Resources([2, 2, 0, 0, 0]));
    let mut av = serde_json::to_value(observe(&a, None)).unwrap();
    let mut bv = serde_json::to_value(observe(&b, None)).unwrap();
    assert_ne!(av["bank"], bv["bank"]);
    av.as_object_mut().unwrap().remove("bank");
    bv.as_object_mut().unwrap().remove("bank");
    assert_eq!(av, bv);
}
