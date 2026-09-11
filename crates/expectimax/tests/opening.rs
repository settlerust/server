//! The opening planner: portfolio pairs, draft prediction, roads, and its public surface.
use settlers_engine::*;
use settlers_expectimax::v2::opening::{self, Bias, OpeningWeights};

fn fresh(seed: u8) -> State {
    State::new(Settings::default(), [seed; 32]).unwrap()
}
fn pips(n: u8) -> u32 {
    if (2..=12).contains(&n) && n != 7 {
        u32::from(6 - n.abs_diff(7))
    } else {
        0
    }
}
fn production(s: &State, v: u8) -> [u32; 5] {
    let mut out = [0; 5];
    for &h in &topology().incident_hexes[v as usize] {
        if let Some(r) = s.layout.resource(h) {
            out[r as usize] += pips(s.layout.number(h));
        }
    }
    out
}
/// Play the whole setup with every seat placing by the planner.
fn play_setup(mut s: State) -> State {
    while s.turn == 0 {
        let p = s.current;
        let action = opening::place(&s, p).expect("a setup action");
        s = apply(&s, p, action).expect("planner actions are legal");
    }
    s
}

#[test]
fn contributions_sum_to_the_value_and_the_ranking_is_sorted() {
    for seed in 0..6u8 {
        let s = fresh(seed);
        let plans = opening::rank_settlements(&s, s.current, 0, &Bias::default());
        assert!(plans.len() >= 6);
        for pair in plans.windows(2) {
            assert!(pair[0].value >= pair[1].value);
        }
        for plan in &plans {
            assert!((plan.terms.total() - plan.value).abs() < 1e-9);
            assert!(plan.terms.draft <= 1e-9, "the draft never adds value");
            assert!(settlement_allowed(&s, s.current, plan.vertex, true));
        }
    }
}

#[test]
fn the_first_seat_expects_the_draft_to_take_something_and_the_last_seat_does_not() {
    let mut first_losses = 0;
    for seed in 0..12u8 {
        let s = fresh(seed);
        let plans = opening::rank_settlements(&s, s.current, 0, &Bias::default());
        if plans.iter().any(|p| p.terms.draft < -0.05) {
            first_losses += 1;
        }
        // Advance to the last seat's first placement: three settlement-and-road pairs.
        let mut t = s;
        for _ in 0..3 {
            let p = t.current;
            t = apply(&t, p, opening::place(&t, p).unwrap()).unwrap();
            let p = t.current;
            t = apply(&t, p, opening::place(&t, p).unwrap()).unwrap();
        }
        assert!(matches!(t.phase, Phase::SetupSettlement { step: 3 }));
        let last = opening::rank_settlements(&t, t.current, 0, &Bias::default());
        for plan in &last {
            assert!(
                plan.terms.draft.abs() < 1e-9,
                "nobody picks between the last seat's two placements"
            );
            assert_eq!(plan.survival, 1.0);
        }
        // The plan's partner is exactly what the second placement then chooses.
        let top = &last[0];
        let p = t.current;
        let mut u = apply(&t, p, Action::BuildSettlement { vertex: top.vertex }).unwrap();
        u = apply(&u, p, opening::place(&u, p).unwrap()).unwrap();
        assert!(matches!(u.phase, Phase::SetupSettlement { step: 4 }));
        let second = opening::rank_settlements(&u, p, 0, &Bias::default());
        assert_eq!(Some(second[0].vertex), top.partner);
    }
    assert!(
        first_losses >= 8,
        "{first_losses} of 12 boards cost the opener something"
    );
}

#[test]
fn planner_pairs_cover_the_island_and_keep_their_pips() {
    let mut total_pips = 0;
    let mut pairs = 0;
    let mut broad = 0;
    for seed in 0..16u8 {
        let s = play_setup(fresh(seed));
        for p in 0..4u8 {
            let mine: Vec<u8> = (0..54u8).filter(|&v| s.board.owner(v) == Some(p)).collect();
            assert_eq!(mine.len(), 2);
            let mut prod = [0u32; 5];
            for &v in &mine {
                for (r, x) in production(&s, v).iter().enumerate() {
                    prod[r] += x;
                }
            }
            let covered = prod.iter().filter(|&&x| x > 0).count();
            assert!(
                covered >= 2,
                "seed {seed} seat {p} covers only {covered} resources: {prod:?}"
            );
            if covered >= 3 {
                broad += 1;
            }
            total_pips += prod.iter().sum::<u32>();
            pairs += 1;
        }
    }
    let mean = f64::from(total_pips) / f64::from(pairs);
    assert!(mean >= 17.0, "mean pair pips {mean:.2}");
    assert!(
        broad * 10 >= pairs * 8,
        "{broad} of {pairs} pairs cover three or more resources"
    );
}

#[test]
fn roads_lead_toward_an_open_site() {
    for seed in 0..8u8 {
        let s = fresh(seed);
        let p = s.current;
        let placed = apply(&s, p, opening::place(&s, p).unwrap()).unwrap();
        let Phase::SetupRoad { vertex, .. } = placed.phase else {
            panic!("expected the road phase");
        };
        let roads = opening::rank_roads(&placed, p, vertex);
        assert!(!roads.is_empty());
        let (edge, value, target) = roads[0];
        assert!(topology().edges[edge as usize].contains(&vertex));
        assert!(value >= 0.0);
        if let Some(w) = target {
            assert!(
                settlement_allowed(&placed, p, w, true),
                "target {w} is open"
            );
            let [a, b] = topology().edges[edge as usize];
            let far = if a == vertex { b } else { a };
            let near = topology().neighbors[far as usize].contains(&w)
                || topology().neighbors[far as usize]
                    .iter()
                    .any(|&x| topology().neighbors[x as usize].contains(&w));
            assert!(near, "target {w} is within two roads of the far end {far}");
        }
    }
}

#[test]
fn an_observation_places_exactly_like_the_state() {
    for seed in 0..6u8 {
        let mut s = fresh(seed);
        for _ in 0..5 {
            let p = s.current;
            let from_state = opening::place(&s, p).unwrap();
            let o = observe(&s, Some(p));
            let rebuilt = opening::from_observation(&o).expect("setup observation");
            assert_eq!(opening::place(&rebuilt, p), Some(from_state));
            s = apply(&s, p, from_state).unwrap();
        }
    }
}

#[test]
fn coverage_alone_and_specialization_are_both_valid_settings() {
    let s = fresh(3);
    let plain = Bias {
        weights: OpeningWeights {
            specialize: 0.0,
            ..OpeningWeights::default()
        },
        ..Bias::default()
    };
    let plans = opening::rank_settlements(&s, s.current, 0, &plain);
    assert!(plans.iter().all(|p| p.strategy == "coverage"));
    let full = Bias {
        weights: OpeningWeights {
            specialize: 1.0,
            ..OpeningWeights::default()
        },
        ..Bias::default()
    };
    let specialized = opening::rank_settlements(&s, s.current, 0, &full);
    assert!(!specialized.is_empty());
    // A specialized valuation never ranks a shared site below its coverage value.
    let mut shared = 0;
    for a in &plans {
        if let Some(same) = specialized.iter().find(|x| x.vertex == a.vertex) {
            assert!(same.value >= a.value - 1e-9, "site {}", a.vertex);
            shared += 1;
        }
    }
    assert!(shared >= 4);
    let text = opening::describe(&plans[0], &s.layout);
    assert!(text.contains("portfolio") && text.contains("pips"));
}
