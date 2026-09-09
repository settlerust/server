mod common;
use common::*;
use settlers_engine::*;

#[test]
fn victory_cards_are_revealed_for_every_seat_only_after_the_finish() {
    let mut state = main_phase();
    dev(&mut state, 0, Development::VictoryPoint);
    dev(&mut state, 1, Development::VictoryPoint);
    dev(&mut state, 1, Development::VictoryPoint);
    state.players[1].development[4] -= 1;
    state.players[1].new_development[4] += 1;
    for seat in [None, Some(0), Some(1)] {
        let observation = observe(&state, seat);
        assert!(
            observation
                .players
                .iter()
                .all(|p| p.victory_cards.is_none())
        );
        assert!(
            !serde_json::to_string(&observation)
                .unwrap()
                .contains("victory_cards")
        );
    }
    state.phase = Phase::Finished { winner: 0 };
    for seat in [None, Some(0), Some(1)] {
        let observation = observe(&state, seat);
        assert_eq!(
            observation
                .players
                .iter()
                .map(|p| p.victory_cards)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(0), Some(0)]
        );
        assert_eq!(observation.players[1].points, points(&state, 1));
        if seat.is_none() {
            assert!(observation.own.is_none());
        }
    }
}
