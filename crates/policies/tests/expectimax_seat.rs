use settlers_engine::*;
use settlers_policies::Policy;
use settlers_policies::expectimax::ExpectimaxSeat;
use settlers_protocol::{Command, EventBatch, GameView, MemberView, PublicEvent};

fn view(s: &State, seat: u8, version: u64) -> GameView {
    GameView {
        id: "seat-test".into(),
        name: "seat test".into(),
        research: false,
        research_hands: None,
        version,
        settings: s.settings,
        host: false,
        members: (0..s.settings.players)
            .map(|i| MemberView {
                idle: false,
                disconnected_at_ms: None,
                resolution: None,
                seat: Some(i),
                name: format!("seat {i}"),
                connected: true,
                ready: true,
                is_you: i == seat,
            })
            .collect(),
        game: Some(observe(s, Some(seat))),
        opening: None,
        offers: vec![],
        filters: None,
        deadline_ms: None,
        discard: None,
    }
}

#[test]
fn specs_parse_and_both_engines_choose_legal_commands() {
    assert!(ExpectimaxSeat::parse("v3").is_err());
    assert!(ExpectimaxSeat::parse("v2:{\"depth\":\"x\"}").is_err());
    for spec in [
        "v1",
        "v2",
        "v1:{\"depth\":1,\"samples\":1}",
        "v2:{\"depth\":1,\"samples\":1}",
    ] {
        let mut seat = ExpectimaxSeat::parse(spec).unwrap();
        let s = State::new(Settings::default(), [3; 32]).unwrap();
        seat.on_batch(&EventBatch {
            sequence: 1,
            timestamp_ms: 0,
            events: vec![PublicEvent::Started],
        });
        let p = s.current;
        let v = view(&s, p, 1);
        let command = seat.choose(&v).expect("setup decision");
        let Command::Act { action } = command else {
            panic!("setup should act");
        };
        assert!(
            apply(&s, p, action).is_ok(),
            "{spec} chose an illegal setup action"
        );
        // Off-turn views without offers yield no command.
        let other = (p + 1) % s.settings.players;
        assert!(seat.choose(&view(&s, other, 1)).is_none());
    }
}
