mod common;
use common::*;
use settlers_engine::*;
#[test]
fn retirement_conserves_cards_preserves_occupancy_and_stops_production_and_turns() {
    let mut s = main_phase();
    hand(&mut s, 1, Resources([3, 4, 2, 1, 0]));
    dev(&mut s, 1, Development::Knight);
    let board = s.board;
    let (mut retired, _) = retire(&s, 1).unwrap();
    assert_eq!(retired.board, board);
    assert_eq!(retired.players[1].resources, Resources::default());
    assert_eq!(retired.players[1].development, [0; 5]);
    assert!(legal_actions(&retired, 1).is_empty());
    assert!(apply(&retired, 1, Action::EndTurn).is_err());
    conservation(&retired);
    for v in 0..54 {
        if board.owner(v) == Some(1) {
            assert!(!settlement_allowed(&retired, 2, v, true));
        }
    }
    for e in 0..72 {
        if board.road(e) == Some(1) {
            assert!(!road_allowed(&retired, 2, e));
        }
    }
    retired = apply(&retired, 0, Action::EndTurn).unwrap();
    assert_eq!(retired.current, 2);
    for total in 2..=12 {
        if total == 7 {
            continue;
        }
        let first = total.min(6);
        let second = total - first;
        if second == 0 {
            continue;
        }
        let produced =
            apply_with_chance(&retired, 2, Action::Roll, Chance::Dice { first, second }).unwrap();
        assert_eq!(produced.players[1].resources, Resources::default());
        conservation(&produced);
    }
}
#[test]
fn retirement_handles_setup_discards_awards_and_last_player() {
    let s = game();
    let (mut s, _) = retire(&s, 0).unwrap();
    assert_eq!(s.current, 1);
    while s.turn == 0 {
        s = apply(&s, s.current, legal_actions(&s, s.current)[0]).unwrap();
    }
    assert_eq!(s.current, 1);
    hand(&mut s, 1, Resources([9, 0, 0, 0, 0]));
    hand(&mut s, 2, Resources([0, 9, 0, 0, 0]));
    s.phase = Phase::Discard {
        pending: (1 << 1) | (1 << 2),
    };
    let (s, _) = retire(&s, 1).unwrap();
    assert_eq!(s.phase, Phase::Discard { pending: 1 << 2 });
    conservation(&s);
    let (s, _) = retire(&s, 2).unwrap();
    assert_eq!(s.phase, Phase::Finished { winner: 3 });
    conservation(&s);
}
