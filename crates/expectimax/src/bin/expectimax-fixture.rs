//! Synthetic public position for native/WASM parity and inference smoke tests.
use settlers_engine::*;
use settlers_expectimax::leaf;
use settlers_protocol::{EventBatch, GameView, PublicEvent};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut s = State::new(
        Settings {
            players: 2,
            ..Settings::default()
        },
        [41; 32],
    )?;
    while s.turn == 0 {
        let action = legal_actions(&s, s.current)
            .into_iter()
            .filter(|a| !matches!(a, Action::RelocateSettlement { .. }))
            .max_by(|a, b| {
                leaf::action_score(&s, s.current, *a)
                    .total_cmp(&leaf::action_score(&s, s.current, *b))
            })
            .ok_or("no setup action")?;
        s = apply(&s, s.current, action)?;
    }
    s.phase = Phase::Main;
    s.current = 0;
    s.turn = 3;
    s.players[0].resources = Resources([0, 2, 0, 2, 2]);
    s.players[1].resources = Resources([0, 0, 0, 0, 1]);
    s.bank = Resources([19, 17, 19, 17, 16]);
    let history = vec![EventBatch {
        sequence: 1,
        timestamp_ms: 0,
        events: vec![
            PublicEvent::Started,
            PublicEvent::Rule {
                event: Event::Produced {
                    player: 0,
                    resources: s.players[0].resources,
                },
            },
            PublicEvent::Rule {
                event: Event::Produced {
                    player: 1,
                    resources: s.players[1].resources,
                },
            },
        ],
    }];
    let view = GameView {
        id: "00000000-0000-4000-8000-000000000000".into(),
        name: "Expectimax smoke fixture".into(),
        research: false,
        research_hands: None,
        version: 1,
        settings: s.settings,
        host: false,
        members: vec![],
        game: Some(observe(&s, Some(0))),
        opening: None,
        offers: vec![],
        filters: None,
        deadline_ms: None,
        discard: None,
    };
    println!(
        "{}",
        serde_json::json!({ "view": view, "history": history })
    );
    Ok(())
}
