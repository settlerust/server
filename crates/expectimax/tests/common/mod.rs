use settlers_engine::*;
use settlers_protocol::{EventBatch, GameView, PublicEvent};

pub fn fixture(players: u8) -> (State, Vec<EventBatch>) {
    let mut state = State::new(
        Settings {
            players,
            ..Settings::default()
        },
        [41; 32],
    )
    .unwrap();
    let mut history = vec![EventBatch {
        sequence: 1,
        timestamp_ms: 0,
        events: vec![PublicEvent::Started],
    }];
    while state.turn == 0 {
        let action = legal_actions(&state, state.current)
            .into_iter()
            .filter(|a| !matches!(a, Action::RelocateSettlement { .. }))
            .max_by(|a, b| {
                settlers_expectimax::leaf::action_score(&state, state.current, *a).total_cmp(
                    &settlers_expectimax::leaf::action_score(&state, state.current, *b),
                )
            })
            .unwrap();
        let (next, events) = apply_recorded(&state, state.current, action).unwrap();
        state = next;
        push(&mut history, events);
    }
    (state, history)
}
pub fn push(history: &mut Vec<EventBatch>, events: Vec<Event>) {
    history.push(EventBatch {
        sequence: history.len() as u64 + 1,
        timestamp_ms: 0,
        events: events
            .into_iter()
            .map(|event| PublicEvent::Rule { event })
            .collect(),
    });
}
pub fn view(s: &State, p: u8, version: u64) -> GameView {
    GameView {
        id: "00000000-0000-4000-8000-000000000000".into(),
        name: "fixture".into(),
        research: false,
        research_hands: None,
        version,
        settings: s.settings,
        host: false,
        members: vec![],
        game: Some(observe(s, Some(p))),
        opening: None,
        offers: vec![],
        filters: None,
        deadline_ms: None,
        discard: None,
    }
}
