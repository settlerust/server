use settlers_engine::*;
use settlers_protocol::{EventBatch, PublicEvent, knowledge::ResourceKnowledge};
fn batch(sequence: u64, events: Vec<PublicEvent>) -> EventBatch {
    EventBatch {
        sequence,
        timestamp_ms: 0,
        events,
    }
}
fn rule(event: Event) -> PublicEvent {
    PublicEvent::Rule { event }
}
fn observation(state: &State) -> Observation {
    let mut state = *state;
    state.bank = Resources(std::array::from_fn(|r| {
        19 - state.players.iter().map(|p| p.resources.0[r]).sum::<u8>()
    }));
    observe(&state, Some(0))
}
#[test]
fn public_receipts_track_exact_hands_until_a_hidden_theft_or_discard() {
    let mut s = State::new(Settings::default(), [9; 32]).unwrap();
    s.players[1].resources = Resources([1, 0, 0, 1, 0]);
    s.players[2].resources = Resources([0, 0, 0, 1, 0]);
    let mut history = vec![
        batch(
            1,
            vec![
                PublicEvent::Started,
                rule(Event::Produced {
                    player: 1,
                    resources: Resources([1, 0, 0, 2, 0]),
                }),
            ],
        ),
        batch(
            2,
            vec![rule(Event::Stole {
                player: 2,
                victim: 1,
                count: 1,
            })],
        ),
    ];
    let o = observation(&s);
    let hidden = ResourceKnowledge::from_history(&o, &history);
    assert_eq!(hidden.players[1].minimum, [0, 0, 0, 1, 0]);
    assert_eq!(hidden.players[1].maximum, [1, 0, 0, 2, 0]);
    assert_eq!(hidden.players[2].maximum, [1, 0, 0, 1, 0]);
    assert!(!hidden.players[2].can_have(Resources::one(Resource::Ore, 1)));
    history[1].events.push(PublicEvent::TheftDetails {
        player: 2,
        victim: 1,
        resource: Resource::Grain,
    });
    let known = ResourceKnowledge::from_history(&o, &history);
    assert_eq!(known.players[1].minimum, s.players[1].resources.0);
    assert!(known.players[1].is_exact());
    assert!(known.players[2].is_exact());
    s.players[1].resources.0[3] = 0;
    history.push(batch(
        3,
        vec![rule(Event::Discarded {
            player: 1,
            count: 1,
        })],
    ));
    let lost = ResourceKnowledge::from_history(&observation(&s), &history);
    // The public bank now reveals that the discarded card was grain.
    assert!(lost.players[1].is_exact());
    assert!((lost.players[1].expected.iter().sum::<f64>() - 1.0).abs() < 1e-9);
}

#[test]
fn snapshot_version_detects_a_missing_tail_even_when_card_totals_still_match() {
    let mut state = State::new(Settings::default(), [9; 32]).unwrap();
    state.players[1].resources = Resources::one(Resource::Ore, 1);
    state.players[2].resources = Resources::one(Resource::Brick, 1);
    let history = vec![batch(
        1,
        vec![
            PublicEvent::Started,
            rule(Event::Produced {
                player: 1,
                resources: Resources::one(Resource::Brick, 1),
            }),
            rule(Event::Produced {
                player: 2,
                resources: Resources::one(Resource::Ore, 1),
            }),
        ],
    )];
    let observation = observation(&state);
    let knowledge = ResourceKnowledge::from_history_at_version(&observation, &history, 2);
    assert!(!knowledge.history_complete);
    assert!(!knowledge.players[1].is_exact());
    assert!(knowledge.players[1].can_have(Resources::one(Resource::Ore, 1)));
    assert!(knowledge.players[0].is_exact());

    let mut with_future = history;
    with_future.push(batch(
        3,
        vec![rule(Event::PlayedDevelopment {
            player: 1,
            card: Development::Knight,
        })],
    ));
    let current = ResourceKnowledge::from_history_at_version(&observation, &with_future, 2);
    assert_eq!(current.used_development, [0; 5]);
}

#[test]
fn bank_supply_tightens_hands_and_abundance_without_assigning_cards_to_a_specific_opponent() {
    let mut state = State::new(Settings::default(), [9; 32]).unwrap();
    state.players[1].resources = Resources::one(Resource::Brick, 4);
    state.players[2].resources = Resources([3, 0, 0, 0, 1]);
    let view = observation(&state);
    let knowledge = ResourceKnowledge::from_history_at_version(&view, &[], 12);
    assert_eq!(knowledge.bank, Resources([12, 19, 19, 19, 18]));
    assert_eq!(knowledge.opponent_totals, Some(Resources([7, 0, 0, 0, 1])));
    for seat in [1, 2] {
        let player = knowledge.players[seat];
        assert_eq!(player.minimum, [3, 0, 0, 0, 0]);
        assert_eq!(player.maximum, [4, 0, 0, 0, 1]);
        assert!(!player.is_exact());
        assert!(!player.can_have(Resources::one(Resource::Lumber, 1)));
        assert!(player.expected[0] > player.expected[4]);
    }
    assert!(
        (knowledge.players[1].expected[4] + knowledge.players[2].expected[4] - 1.0).abs() < 1e-9
    );
}
