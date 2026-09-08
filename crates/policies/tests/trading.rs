use settlers_engine::*;
use settlers_policies::{Policy, fast_builder::FastBuilder, trading::TradingPolicy};
use settlers_protocol::*;
#[test]
fn remote_policy_proposes_affordable_disjoint_swap_once_per_turn() {
    let mut state = State::new(Settings::default(), [4; 32]).unwrap();
    while state.turn == 0 {
        state = timeout(&state).unwrap().0;
    }
    state.phase = Phase::Main;
    for player in &mut state.players {
        player.resources = Resources([4, 0, 0, 0, 0]);
    }
    let mut policy = TradingPolicy::new(Box::new(FastBuilder));
    let view = GameView {
        research: false,
        research_hands: None,
        id: "trade".into(),
        name: "trade".into(),
        version: 1,
        settings: state.settings,
        host: false,
        members: vec![],
        game: Some(observe(&state, Some(state.current))),
        opening: None,
        offers: vec![],
        filters: None,
        deadline_ms: None,
        discard: None,
    };
    let Some(Command::OfferTrade {
        give, receive, to, ..
    }) = policy.choose(&view)
    else {
        panic!("expected an offer before bank trade/end turn");
    };
    assert!(
        state.players[state.current as usize]
            .resources
            .contains(give)
    );
    assert!(give.disjoint(receive));
    assert_eq!(give.total(), 1);
    assert_eq!(receive.total(), 1);
    assert_eq!(to.len(), 3);
    assert!(!to.contains(&state.current));
    assert!(!matches!(
        policy.choose(&view),
        Some(Command::OfferTrade { .. })
    ));
    let mut spectator = view.clone();
    spectator.game = Some(observe(&state, None));
    assert!(
        TradingPolicy::new(Box::new(FastBuilder))
            .choose(&spectator)
            .is_none()
    );
}
