use settlers_engine::{Action, Observation, Settings, State, observe};
use settlers_policies::Policy;
use settlers_protocol::{Command, Communication, GameView, Intent, PublicEvent};

struct ConversationalPolicy {
    heard: bool,
    replied: bool,
}
impl Policy for ConversationalPolicy {
    fn name(&self) -> &'static str {
        "conversation-test"
    }
    fn choose_action(&mut self, observation: &Observation) -> Option<Action> {
        observation.legal_actions.first().copied()
    }
    fn on_event(&mut self, event: &PublicEvent) {
        self.heard |= matches!(event, PublicEvent::Message { .. });
    }
    fn conversation(&mut self, _: &Observation) -> Option<Communication> {
        if !self.heard || self.replied {
            return None;
        }
        self.replied = true;
        Some(Communication {
            intent: Intent::Strategy,
            resources: None,
            text: Some("Building toward grain".into()),
        })
    }
}
#[test]
fn conversation_hook_receives_structured_events_and_then_resumes_game_decisions() {
    let state = State::new(Settings::default(), [7; 32]).unwrap();
    let view = GameView {
        research: false,
        research_hands: None,
        id: "test".into(),
        name: "test".into(),
        version: 1,
        settings: state.settings,
        host: true,
        members: vec![],
        game: Some(observe(&state, Some(state.current))),
        offers: vec![],
        opening: None,
        filters: None,
        deadline_ms: None,
        discard: None,
    };
    let mut policy = ConversationalPolicy {
        heard: false,
        replied: false,
    };
    policy.on_event(&PublicEvent::Message {
        from: (state.current + 1) % 4,
        to: None,
        message: Communication {
            intent: Intent::Chat,
            resources: None,
            text: Some("What is your plan?".into()),
        },
    });
    assert!(matches!(
        policy.choose(&view),
        Some(Command::Communicate {
            message: Communication {
                intent: Intent::Strategy,
                ..
            },
            ..
        })
    ));
    assert!(matches!(
        policy.choose(&view),
        Some(Command::Act {
            action: Action::BuildSettlement { .. }
        })
    ));
}
