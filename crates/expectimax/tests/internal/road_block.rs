//! Mechanism test for the `opponent_expansion` leaf term: a road that cuts the leading
//! expander's shortest path to their best site lowers the denial term, and the candidate
//! generator ranks such a road strictly higher once the term is on. The board is a
//! synthetic fixture, not a recorded game.
use super::*;
use crate::v2::Weights;

fn edge_between(u: u8, v: u8) -> Option<u8> {
    topology().incident_edges[u as usize]
        .iter()
        .copied()
        .find(|&e| topology().edges[e as usize].contains(&v))
}

/// A board with seat 0 settled at `d` with a road to `b`, and seat 1 settled at `a`; the
/// candidate block is seat 0's road on the edge (a, b).
fn board_state(a: u8, b: u8, d: u8) -> (State, u8) {
    let mut buildings = [0u8; 54];
    buildings[d as usize] = 1; // seat 0 settlement
    buildings[a as usize] = 2; // seat 1 settlement
    let mut roads = [None; 72];
    let own = edge_between(d, b).unwrap();
    roads[own as usize] = Some(0);
    let mut state = State::new(Settings::default(), [23; 32]).unwrap();
    state.board = Board::from_public(&buildings, &roads).unwrap();
    state.phase = Phase::Main;
    state.turn = 8;
    state.current = 0;
    state.players[0].settlements = 1;
    state.players[0].roads = 1;
    state.players[0].resources = Resources([5; 5]);
    state.players[1].settlements = 1;
    (state, edge_between(a, b).unwrap())
}

/// The candidate score of building `edge`, with the given leaf weights.
fn score_of(state: &State, edge: u8, weights: Weights) -> f64 {
    let next = apply(state, 0, Action::BuildRoad { edge }).unwrap();
    let base = road_base(state, 0, &weights);
    road_score(state, &next, 0, &weights, &base)
}

/// Find (a, b, c, d) where cutting the edge (a, b) strictly lowers seat 1's best
/// discounted reachable-site value while leaving seat 0's own reach untouched.
fn blocking_fixture() -> (State, u8) {
    for a in 0..54u8 {
        for &b in &topology().neighbors[a as usize] {
            for &c in &topology().neighbors[b as usize] {
                if c == a {
                    continue;
                }
                for &d in &topology().neighbors[b as usize] {
                    if d == a || d == c {
                        continue;
                    }
                    let (state, edge) = board_state(a, b, d);
                    if !road_allowed(&state, 0, edge) {
                        continue;
                    }
                    let Ok(next) = apply(&state, 0, Action::BuildRoad { edge }) else {
                        continue;
                    };
                    if crate::v2::leaf::reach(&state, 0) != crate::v2::leaf::reach(&next, 0) {
                        continue;
                    }
                    let denied_before = crate::v2::leaf::opponent_reach(&state, 0);
                    let denied_after = crate::v2::leaf::opponent_reach(&next, 0);
                    if denied_before > denied_after + 1e-9 {
                        return (state, edge);
                    }
                }
            }
        }
    }
    panic!("no blocking fixture found on this board");
}

#[test]
fn blocking_road_is_valued_and_ranked_only_with_the_term_on() {
    let (state, edge) = blocking_fixture();
    let road = Action::BuildRoad { edge };
    let next = apply(&state, 0, road).unwrap();
    // The mechanism: the road removes the edge from the opponent's path graph, and the
    // leading opponent's discounted best-site value falls.
    let denied_before = crate::v2::leaf::opponent_reach(&state, 0);
    let denied_after = crate::v2::leaf::opponent_reach(&next, 0);
    assert!(denied_before > denied_after + 1e-9);
    // The valuation: the leaf carries the denial over the remaining rolls, scaled by the
    // weight, and the default leaf has no such term.
    let on_weights = Weights {
        opponent_expansion: 0.5,
        ..Weights::default()
    };
    let base = crate::v2::leaf::evaluate(&state, 0, &Weights::default());
    assert!(!base.0.contains_key("opponent_expansion"));
    let valued = crate::v2::leaf::evaluate(&state, 0, &on_weights);
    let future = on_weights.card_value * crate::v2::leaf::remaining_rolls(&state, &on_weights);
    let expected = -denied_before * future * 0.5;
    let actual = *valued.0.get("opponent_expansion").unwrap();
    assert!((actual - expected).abs() < 1e-9);
    // The enumeration: the candidate score of the road rises by exactly the weighted
    // denial, so a blocking road enters the trimmed candidate list when the cut is large.
    let off_score = score_of(&state, edge, Weights::default());
    let on_score = score_of(&state, edge, on_weights);
    let credit = 0.5 * (denied_before - denied_after);
    assert!((on_score - off_score - credit).abs() < 1e-9);
    assert!(credit > 0.0);
}
