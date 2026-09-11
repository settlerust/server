//! Mechanism test for the `road_contention_gate` weight: the half-strength contention
//! credit and the contested-trail road bonus apply only while the seat ties or leads the
//! table in brick plus lumber production. The boards are synthetic fixtures.
use super::*;
use crate::v2::weights::Weights;

/// A simple path of `depth` edges starting at `start`, avoiding banned vertices.
fn find_path(
    start: u8,
    banned: &[bool; 54],
    visited: &mut [bool; 54],
    used: &mut [bool; 72],
    out: &mut Vec<u8>,
    depth: usize,
) -> bool {
    if out.len() == depth {
        return true;
    }
    for &e in &topology().incident_edges[start as usize] {
        if used[e as usize] {
            continue;
        }
        let [a, b] = topology().edges[e as usize];
        let next = if a == start { b } else { a };
        if banned[next as usize] || visited[next as usize] {
            continue;
        }
        used[e as usize] = true;
        visited[next as usize] = true;
        out.push(e);
        if find_path(next, banned, visited, used, out, depth) {
            return true;
        }
        out.pop();
        used[e as usize] = false;
        visited[next as usize] = false;
    }
    false
}

/// Whether the vertex touches a brick or lumber hex.
fn touches_road_resources(s: &State, v: u8) -> bool {
    topology().incident_hexes[v as usize].iter().any(|&h| {
        matches!(
            s.layout.resource(h),
            Some(Resource::Brick | Resource::Lumber)
        )
    })
}

/// A board with a settlement and a four-road trail for each of seats 0 and 1.
fn race_state(x0: u8, path0: &[u8], x1: u8, path1: &[u8]) -> State {
    let mut buildings = [0u8; 54];
    buildings[x0 as usize] = 1; // seat 0 settlement
    buildings[x1 as usize] = 2; // seat 1 settlement
    let mut roads = [None; 72];
    for &e in path0 {
        roads[e as usize] = Some(0);
    }
    for &e in path1 {
        roads[e as usize] = Some(1);
    }
    let mut state = State::new(Settings::default(), [31; 32]).unwrap();
    state.board = Board::from_public(&buildings, &roads).unwrap();
    state.phase = Phase::Main;
    state.turn = 8;
    state.current = 0;
    state.players[0].settlements = 1;
    state.players[0].roads = 4;
    state.players[0].resources = Resources([5; 5]);
    state.players[1].settlements = 1;
    state.players[1].roads = 4;
    state
}

/// The vertices a path visits, including its start.
fn path_vertices(start: u8, path: &[u8]) -> Vec<u8> {
    let mut out = vec![start];
    let mut cur = start;
    for &e in path {
        let [a, b] = topology().edges[e as usize];
        cur = if a == cur { b } else { a };
        out.push(cur);
    }
    out
}

/// One fixture where seat 0 leads in brick plus lumber and one where it does not. Both
/// have seat 0 within one road of the award leader.
fn race_fixtures() -> (State, State) {
    let mut lead = None;
    let mut trail = None;
    'outer: for x0 in 0..54u8 {
        let mut used = [false; 72];
        let mut visited = [false; 54];
        visited[x0 as usize] = true;
        let mut path0 = Vec::new();
        if !find_path(x0, &[false; 54], &mut visited, &mut used, &mut path0, 4) {
            continue;
        }
        let taken: Vec<bool> = {
            let mut t = vec![false; 54];
            for v in path_vertices(x0, &path0) {
                t[v as usize] = true;
            }
            t
        };
        for x1 in 0..54u8 {
            if taken[x1 as usize] {
                continue;
            }
            let mut banned = [false; 54];
            for (v, &t) in taken.iter().enumerate() {
                banned[v] = t;
            }
            banned[x0 as usize] = true;
            let mut used1 = [false; 72];
            for &e in &path0 {
                used1[e as usize] = true;
            }
            let mut visited1 = [false; 54];
            visited1[x1 as usize] = true;
            let mut path1 = Vec::new();
            if !find_path(x1, &banned, &mut visited1, &mut used1, &mut path1, 4) {
                continue;
            }
            let state = race_state(x0, &path0, x1, &path1);
            let own0 = touches_road_resources(&state, x0);
            let own1 = touches_road_resources(&state, x1);
            if own0 && !own1 && lead.is_none() {
                lead = Some(state);
            }
            if !own0 && own1 && trail.is_none() {
                trail = Some(state);
            }
            if lead.is_some() && trail.is_some() {
                break 'outer;
            }
        }
    }
    (
        lead.expect("no lead fixture on this board"),
        trail.expect("no trailing fixture on this board"),
    )
}

/// An edge that extends seat 0's trail.
fn extending_edge(s: &State) -> u8 {
    let base = road_length(s, 0);
    (0..72u8)
        .find(|&e| {
            road_allowed(s, 0, e)
                && apply(s, 0, Action::BuildRoad { edge: e })
                    .is_ok_and(|next| road_length(&next, 0) > base)
        })
        .expect("no extending edge in fixture")
}

#[test]
fn contention_credit_requires_the_road_production_lead_when_gated() {
    let (lead, trailing) = race_fixtures();
    for (state, leads) in [(&lead, true), (&trailing, false)] {
        // The race precondition: four roads each, seat 0 within one road of the leader.
        let length = road_length(state, 0);
        let leader = road_length(state, 1).max(4);
        assert!(length >= 4 && length + 1 > leader);
        assert_eq!(crate::v2::leaf::road_production_lead(state, 0), leads);
        let base = Weights::default();
        let gated = Weights {
            road_contention_gate: 1.0,
            ..Weights::default()
        };
        let road_term = |w: &Weights| crate::v2::leaf::evaluate(state, 0, w).0["road"];
        let credit = base.road_contention * 0.5;
        let expected_base = f64::from(length) * base.road_length + credit;
        assert_eq!(road_term(&base), expected_base);
        let expected_gated =
            f64::from(length) * base.road_length + if leads { credit } else { 0.0 };
        assert_eq!(road_term(&gated), expected_gated);
        // The enumeration side: the +0.5 bonus for a trail-extending road follows the
        // same gate.
        let edge = extending_edge(state);
        let next = apply(state, 0, Action::BuildRoad { edge }).unwrap();
        let score = |w: Weights| {
            let base = road_base(state, 0, &w);
            road_score(state, &next, 0, &w, &base)
        };
        let bonus_gap = score(base) - score(gated);
        let expected_gap = if leads { 0.0 } else { 0.5 };
        assert!(
            (bonus_gap - expected_gap).abs() < 1e-9,
            "trail bonus gap {bonus_gap}, expected {expected_gap} (leads {leads})"
        );
    }
}
