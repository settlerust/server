use settlers_engine::*;
#[test]
fn public_board_roundtrips_and_rejects_invalid_packing() {
    let s = State::new(Settings::default(), [3; 32]).unwrap();
    let a = legal_actions(&s, s.current)[0];
    let s = apply(&s, s.current, a).unwrap();
    let o = observe(&s, None);
    assert_eq!(Board::from_public(&o.buildings, &o.roads).unwrap(), s.board);
    assert!(Board::from_public(&[0; 53], &o.roads).is_err());
    assert!(Board::from_public(&[9; 54], &o.roads).is_err());
    assert!(Board::from_public(&o.buildings, &[Some(4); 72]).is_err());
}
