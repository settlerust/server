mod common;
use common::*;
use settlers_engine::*;
#[test]
fn setup_uses_snake_order_and_grants_only_second_settlement_resources() {
    for n in 2..=4 {
        let mut s = State::new(
            Settings {
                players: n,
                ..Settings::default()
            },
            [9; 32],
        )
        .unwrap();
        let first = s.first;
        let mut order = vec![];
        while s.turn == 0 {
            let step = match s.phase {
                Phase::SetupSettlement { step } => step,
                _ => panic!(),
            };
            order.push(s.current);
            let p = s.current;
            let v = match legal_actions(&s, p)[0] {
                Action::BuildSettlement { vertex } => vertex,
                _ => panic!(),
            };
            s = apply(&s, p, Action::BuildSettlement { vertex: v }).unwrap();
            assert_eq!(
                s.players[p as usize].resources.total(),
                if step < n {
                    0
                } else {
                    topology().incident_hexes[v as usize]
                        .iter()
                        .filter(|&&h| s.layout.resource(h).is_some())
                        .count() as u16
                }
            );
            let a = legal_actions(&s, p)[0];
            s = apply(&s, p, a).unwrap();
            conservation(&s);
        }
        assert_eq!(
            order,
            (0..n)
                .chain((0..n).rev())
                .map(|p| (p + first) % n)
                .collect::<Vec<_>>()
        );
        assert_eq!(s.current, first);
        assert_eq!(s.phase, Phase::Roll);
    }
}
#[test]
fn adjacent_and_disconnected_settlements_are_rejected() {
    let mut s = game();
    s = apply(&s, 0, Action::BuildSettlement { vertex: 0 }).unwrap();
    s = apply(&s, 0, legal_actions(&s, 0)[0]).unwrap();
    let adjacent = topology().neighbors[0][0];
    assert_eq!(
        apply(&s, 1, Action::BuildSettlement { vertex: adjacent }),
        Err(RuleError::IllegalPlacement)
    );
    s.phase = Phase::Main;
    assert_eq!(
        apply(&s, 1, Action::BuildSettlement { vertex: 53 }),
        Err(RuleError::IllegalPlacement)
    );
}
#[test]
fn setup_road_must_touch_just_placed_settlement() {
    let s = apply(&game(), 0, Action::BuildSettlement { vertex: 0 }).unwrap();
    let edge = (0..72)
        .find(|&e| !topology().edges[e as usize].contains(&0))
        .unwrap();
    assert_eq!(
        apply(&s, 0, Action::BuildRoad { edge }),
        Err(RuleError::IllegalPlacement)
    );
}
#[test]
fn city_cost_and_piece_return_are_exact() {
    let mut s = main_phase();
    hand(&mut s, 0, Resources::CITY);
    let v = (0..54).find(|&v| s.board.building(v) == 1).unwrap();
    let next = apply(&s, 0, Action::BuildCity { vertex: v }).unwrap();
    assert_eq!(next.players[0].cities, 1);
    assert_eq!(next.players[0].settlements, 1);
    assert_eq!(next.players[0].resources, Resources::default());
    assert_eq!(next.board.building(v), 5);
    conservation(&next);
}
#[test]
fn failed_action_does_not_mutate_state() {
    let s = main_phase();
    let before = s;
    assert!(apply(&s, 0, Action::BuildCity { vertex: 255 }).is_err());
    assert_eq!(s, before);
    assert_eq!(apply(&s, 4, Action::Roll), Err(RuleError::InvalidPlayer));
}
#[test]
fn opponent_building_blocks_road_extension() {
    let mut s = main_phase();
    let [a, b] = topology().edges[0];
    s.board = board(&[(b, 2)], &[(0, 0)]);
    let e = topology().incident_edges[b as usize]
        .iter()
        .copied()
        .find(|&e| e != 0)
        .unwrap();
    assert!(!road_allowed(&s, 0, e));
    assert!(
        topology().incident_edges[a as usize]
            .iter()
            .any(|&e| e != 0 && road_allowed(&s, 0, e))
    );
}

#[test]
fn physical_piece_limits_reject_otherwise_legal_builds_without_payment() {
    let mut s = main_phase();
    hand(&mut s, 0, Resources([2, 2, 2, 3, 4]));
    let e = (0..72).find(|&e| road_allowed(&s, 0, e)).unwrap();
    s.players[0].roads = 15;
    assert_eq!(
        apply(&s, 0, Action::BuildRoad { edge: e }),
        Err(RuleError::NoPieces)
    );
    let v = (0..54).find(|&v| s.board.building(v) == 1).unwrap();
    s.players[0].cities = 4;
    assert_eq!(
        apply(&s, 0, Action::BuildCity { vertex: v }),
        Err(RuleError::NoPieces)
    );
    let mut setup = game();
    setup.players[0].settlements = 5;
    assert_eq!(
        apply(&setup, 0, Action::BuildSettlement { vertex: 0 }),
        Err(RuleError::NoPieces)
    );
}
