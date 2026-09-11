//! Probe for the trail-breaking hypothesis: whether a settlement that cuts an opponent's
//! longest road is enumerated as a root candidate and credited for the lost award. The
//! board is a synthetic fixture, not a recorded game.
#[allow(dead_code)]
mod common;
use settlers_engine::*;
use settlers_expectimax::v2::{SearchConfig, analyze};

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

/// A simple path of `depth` edges from `start`, avoiding banned vertices.
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

/// Seat 1 holds a five-road trail through `cut`; seat 0 has a road ending at `cut` and
/// can settle there.
fn break_fixture() -> (State, u8) {
    for x1 in 0..54u8 {
        let mut used = [false; 72];
        let mut visited = [false; 54];
        visited[x1 as usize] = true;
        let mut path = Vec::new();
        if !find_path(x1, &[false; 54], &mut visited, &mut used, &mut path, 5) {
            continue;
        }
        // Vertices along the trail; the break goes at an interior vertex.
        let mut verts = vec![x1];
        let mut cur = x1;
        for &e in &path {
            let [a, b] = topology().edges[e as usize];
            cur = if a == cur { b } else { a };
            verts.push(cur);
        }
        let cut = verts[2];
        // Seat 0 reaches `cut` along a two-road approach d' -> d -> cut; the settlement
        // sits at d' so that every neighbour of `cut` stays empty and the break is legal.
        for &e2 in &topology().incident_edges[cut as usize] {
            if path.contains(&e2) {
                continue;
            }
            let [a, b] = topology().edges[e2 as usize];
            let d = if a == cut { b } else { a };
            if verts.contains(&d) {
                continue;
            }
            for &e1 in &topology().incident_edges[d as usize] {
                if e1 == e2 || path.contains(&e1) {
                    continue;
                }
                let [a, b] = topology().edges[e1 as usize];
                let dp = if a == d { b } else { a };
                if dp == cut || verts.contains(&dp) {
                    continue;
                }
                let mut buildings = [0u8; 54];
                buildings[x1 as usize] = 2; // seat 1 settlement
                buildings[dp as usize] = 1; // seat 0 settlement
                if topology().neighbors[dp as usize]
                    .iter()
                    .any(|&n| buildings[n as usize] != 0 && n != dp)
                {
                    continue; // keep seat 0's settlement legal
                }
                let mut roads = [None; 72];
                for &e in &path {
                    roads[e as usize] = Some(1);
                }
                roads[e1 as usize] = Some(0);
                roads[e2 as usize] = Some(0);
                let mut state = State::new(Settings::default(), [41; 32]).unwrap();
                state.board = Board::from_public(&buildings, &roads).unwrap();
                state.phase = Phase::Main;
                state.turn = 8;
                state.current = 0;
                state.longest_road = Some(1);
                state.players[0].settlements = 1;
                state.players[0].roads = 2;
                state.players[0].resources = Resources([5; 5]);
                state.bank = Resources([14; 5]); // 19 less seat 0's hand
                state.players[1].settlements = 1;
                state.players[1].roads = 5;
                if !settlement_allowed(&state, 0, cut, false) {
                    continue;
                }
                return (state, cut);
            }
        }
    }
    panic!("no trail-break fixture found on this board");
}

#[test]
fn trail_break_is_enumerated_and_credited() {
    let (state, cut) = break_fixture();
    assert_eq!(road_length(&state, 1), 5);
    let view = common::view(&state, 0, 1);
    let history = vec![settlers_protocol::EventBatch {
        sequence: 1,
        timestamp_ms: 0,
        events: vec![settlers_protocol::PublicEvent::Started],
    }];
    let report = analyze(&view, &history, &quick(), None, &|| 0.0).unwrap();
    let break_action = Action::BuildSettlement { vertex: cut };
    let rank = report.candidates.iter().position(|c| {
        c.command
            == Some(settlers_protocol::Command::Act {
                action: break_action,
            })
    });
    let Some(rank) = rank else {
        panic!(
            "trail break not enumerated; candidates: {:?}",
            report
                .candidates
                .iter()
                .take(8)
                .map(|c| c.command.clone())
                .collect::<Vec<_>>()
        );
    };
    // The break cuts the trail below the award minimum and the leaf credits the lost
    // award through the opponent-points term.
    let after = apply(&state, 0, break_action).unwrap();
    assert!(road_length(&after, 1) <= 3);
    assert_eq!(after.longest_road, None);
    let before_terms = settlers_expectimax::v2::leaf::evaluate(
        &state,
        0,
        &settlers_expectimax::v2::Weights::default(),
    );
    let after_terms = settlers_expectimax::v2::leaf::evaluate(
        &after,
        0,
        &settlers_expectimax::v2::Weights::default(),
    );
    assert!(
        after_terms.0["opponent_progress"] > before_terms.0["opponent_progress"],
        "the break must lower the opponent-points term"
    );
    eprintln!(
        "trail break candidate rank {rank} of {} (value {:.0}, top {:.0})",
        report.candidates.len(),
        report.candidates[rank].expected_value,
        report.candidates[0].expected_value,
    );
}
