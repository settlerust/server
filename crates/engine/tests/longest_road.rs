mod common;
use common::*;
use settlers_engine::*;
fn hex_roads(h: usize) -> Vec<u8> {
    let vs = topology().hex_vertices[h];
    (0..6)
        .map(|i| {
            topology()
                .edges
                .iter()
                .position(|e| e.contains(&vs[i]) && e.contains(&vs[(i + 1) % 6]))
                .unwrap() as u8
        })
        .collect()
}
#[test]
fn a_cycle_counts_every_edge_and_an_opponent_cuts_transit() {
    let mut s = main_phase();
    let es = hex_roads(9);
    s.board = board(&[], &es.iter().map(|&e| (e, 0)).collect::<Vec<_>>());
    assert_eq!(road_length(&s, 0), 6);
    let v = topology().hex_vertices[9];
    s.board = board(
        &[(v[0], 2), (v[3], 3)],
        &es.iter().map(|&e| (e, 0)).collect::<Vec<_>>(),
    );
    assert_eq!(road_length(&s, 0), 3);
}
#[test]
fn a_fork_cannot_count_three_branches_in_one_trail() {
    let mut s = main_phase();
    let v = (0..54)
        .find(|&v| topology().incident_edges[v].len() == 3)
        .unwrap();
    let es = &topology().incident_edges[v];
    s.board = board(&[], &es.iter().map(|&e| (e, 0)).collect::<Vec<_>>());
    assert_eq!(road_length(&s, 0), 2);
}
#[test]
fn gaining_points_on_another_turn_does_not_win_until_own_turn() {
    let mut s = main_phase();
    s.settings.points_to_win = 3;
    dev(&mut s, 1, Development::VictoryPoint);
    assert_eq!(points(&s, 1), 3);
    let n = apply(&s, 0, Action::EndTurn).unwrap();
    assert_eq!(n.phase, Phase::Finished { winner: 1 });
}

#[test]
fn longest_road_tie_preserves_holder_but_without_holder_awards_neither() {
    let mut s = main_phase();
    let mut roads: Vec<_> = hex_roads(0).into_iter().map(|e| (e, 0)).collect();
    roads.extend(hex_roads(18).into_iter().map(|e| (e, 1)));
    let v = topology().hex_vertices[9][0];
    let e = topology().incident_edges[v as usize][0];
    assert!(!roads.iter().any(|&(edge, _)| edge == e));
    roads.push((e, 2));
    s.board = board(&[], &roads);
    s.current = 2;
    hand(&mut s, 2, Resources::SETTLEMENT);
    for holder in [None, Some(0), Some(1)] {
        s.longest_road = holder;
        let n = apply(&s, 2, Action::BuildSettlement { vertex: v }).unwrap();
        assert_eq!(road_length(&n, 0), 6);
        assert_eq!(road_length(&n, 1), 6);
        assert_eq!(n.longest_road, holder);
    }
}
