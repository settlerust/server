#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::search::{Limits, Search, World};
use settlers_expectimax::v2::{Weights, opponent};

/// Turn planning must explore action sequences, not only the first action.
#[test]
fn turn_planning_explores_sequences_when_several_actions_are_available() {
    let (mut s, _) = common::fixture(4);
    let never = || false;
    let mut checked = 0;
    while s.turn < 80 && checked < 6 {
        let p = match s.phase {
            Phase::Discard { pending } => pending.trailing_zeros() as u8,
            _ => s.current,
        };
        if s.phase == Phase::Main && s.turn > 8 {
            let seat = p;
            let mut search = Search::new(
                seat,
                Weights::default(),
                Limits {
                    depth: 1,
                    beam: 4,
                    root_beam: 4,
                    per_candidate: 1,
                    afterstates: 96,
                    scenarios: 4,
                    inner_scenarios: 2,
                    node_budget: 1_000_000,
                },
                &never,
            );
            let cands = search.candidates(&s, seat, 0);
            if cands.len() >= 3 {
                let (value, line) = search
                    .best_line(
                        vec![World {
                            state: s,
                            weight: 1.0,
                        }],
                        1,
                        0,
                    )
                    .unwrap();
                assert!(value.is_finite());
                assert!(!line.is_empty());
                assert!(
                    search.stats.own_nodes >= 2 && search.stats.leaves >= 2,
                    "turn {} explored {} nodes and {} leaves with {} candidates",
                    s.turn,
                    search.stats.own_nodes,
                    search.stats.leaves,
                    cands.len()
                );
                checked += 1;
            }
        }
        let a = opponent::act(&s, p, 0);
        s = match apply(&s, p, a) {
            Ok(n) => n,
            Err(_) => apply(&s, p, *legal_actions(&s, p).first().unwrap()).unwrap(),
        };
        if matches!(s.phase, Phase::Finished { .. }) {
            break;
        }
    }
    assert!(checked >= 3);
}
