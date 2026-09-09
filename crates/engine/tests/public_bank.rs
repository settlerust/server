use settlers_engine::*;

#[test]
fn every_view_exposes_the_exact_bank_while_private_hands_stay_seat_scoped() {
    let mut state = State::new(Settings::default(), [42; 32]).unwrap();
    state.players[0].resources = Resources([1, 2, 3, 4, 5]);
    state.players[1].resources = Resources([6, 5, 4, 3, 2]);
    state.bank = Resources([12; 5]);
    for seat in [None, Some(0), Some(1), Some(2), Some(3)] {
        let view = observe(&state, seat);
        assert_eq!(view.bank, state.bank);
        assert_eq!(view.bank_available, [true; 5]);
        assert_eq!(view.own, seat.map(|p| state.players[p as usize]));
        let json = serde_json::to_value(&view).unwrap();
        assert!(json.get("random").is_none());
        assert!(json.get("deck").is_none());
        assert!(
            json["players"]
                .as_array()
                .unwrap()
                .iter()
                .all(|player| player.get("resources").is_none())
        );
    }
}
