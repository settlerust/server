mod common;
use common::*;
use settlers_engine::*;

#[test]
fn relocation_returns_starting_cards_and_keeps_the_same_setup_step() {
    let mut state = game();
    while !matches!(state.phase, Phase::SetupSettlement { step } if step == state.settings.players)
    {
        state = apply(
            &state,
            state.current,
            legal_actions(&state, state.current)[0],
        )
        .unwrap();
    }
    let player = state.current;
    let placed = apply(&state, player, legal_actions(&state, player)[0]).unwrap();
    let Phase::SetupRoad { step, vertex: from } = placed.phase else {
        panic!()
    };
    let relocation = legal_actions(&placed, player)
        .into_iter()
        .find(|a| matches!(a, Action::RelocateSettlement { .. }))
        .unwrap();
    let Action::RelocateSettlement { vertex } = relocation else {
        panic!()
    };
    let (moved, events) = apply_recorded(&placed, player, relocation).unwrap();
    let direct = apply(&state, player, Action::BuildSettlement { vertex }).unwrap();
    assert_eq!(moved, direct);
    assert_eq!(moved.phase, Phase::SetupRoad { step, vertex });
    assert!(events.iter().any(
        |event| matches!(event, Event::SetupSettlementMoved { from: old, .. } if *old == from)
    ));
    conservation(&moved);
    let back = apply(&moved, player, Action::RelocateSettlement { vertex: from }).unwrap();
    assert_eq!(back, placed);
    let committed = apply(&back, player, legal_actions(&back, player)[0]).unwrap();
    assert!(apply(&committed, player, relocation).is_err());
    assert!(apply(&placed, (player + 1) % 4, relocation).is_err());
    assert!(apply(&placed, player, Action::RelocateSettlement { vertex: 255 }).is_err());
}
