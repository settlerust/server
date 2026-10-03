use settlers_engine::*;

fn initial(players: u8) -> State {
    State::new(
        Settings {
            players,
            ..Settings::default()
        },
        [9; 32],
    )
    .unwrap()
}
#[test]
fn invalid_indices_and_inventories_are_rejected_without_panicking() {
    let original = initial(2);
    let cases: Vec<(&str, State)> = vec![
        ("settings", {
            let mut s = original;
            s.settings.players = 5;
            s
        }),
        ("turn", {
            let mut s = original;
            s.current = 2;
            s
        }),
        ("first", {
            let mut s = original;
            s.first = 255;
            s
        }),
        ("port", {
            let mut s = original;
            s.layout.ports[0] = 255;
            s
        }),
        ("hex", {
            let mut s = original;
            s.layout.hexes[0] = 255;
            s
        }),
        ("robber", {
            let mut s = original;
            s.robber = 19;
            s
        }),
        ("retired", {
            let mut s = original;
            s.retired = 128;
            s
        }),
        ("award", {
            let mut s = original;
            s.longest_road = Some(2);
            s
        }),
        ("setup", {
            let mut s = original;
            s.phase = Phase::SetupSettlement { step: 4 };
            s
        }),
        ("vertex", {
            let mut s = original;
            s.phase = Phase::SetupRoad {
                step: 0,
                vertex: 54,
            };
            s
        }),
        ("discard", {
            let mut s = original;
            s.phase = Phase::Discard { pending: 4 };
            s
        }),
        ("empty discard", {
            let mut s = original;
            s.phase = Phase::Discard { pending: 0 };
            s
        }),
        ("free roads", {
            let mut s = original;
            s.phase = Phase::FreeRoads {
                remaining: 3,
                resume: Resume::Main,
            };
            s
        }),
        ("winner", {
            let mut s = original;
            s.phase = Phase::Finished { winner: 2 };
            s
        }),
        ("resource", {
            let mut s = original;
            s.bank.0[0] = 20;
            s
        }),
        ("deck", {
            let mut s = original;
            s.deck[0] = 13;
            s
        }),
        ("new card", {
            let mut s = original;
            s.players[0].new_development[4] = 1;
            s
        }),
        ("used card", {
            let mut s = original;
            s.used_development[0] = 255;
            s
        }),
        ("pieces", {
            let mut s = original;
            s.players[0].roads = 1;
            s
        }),
        ("inactive", {
            let mut s = original;
            s.bank.0[0] -= 1;
            s.players[2].resources.0[0] = 1;
            s
        }),
    ];
    for (name, state) in cases {
        assert!(state.validate().is_err(), "{name}");
    }
    for (field, value) in [("vertices", 255), ("roads", 255)] {
        let mut json = serde_json::to_value(original).unwrap();
        json["board"][field][0] = value.into();
        let state: State = serde_json::from_value(json).unwrap();
        assert!(state.validate().is_err());
    }
    assert_eq!(original.validate(), Ok(()));
}
#[test]
fn legal_transitions_and_retirement_preserve_valid_state() {
    for players in 2..=4 {
        for seed in 0..8 {
            let mut state = State::new(
                Settings {
                    players,
                    ..Settings::default()
                },
                [seed; 32],
            )
            .unwrap();
            for step in 0..400 {
                state.validate().unwrap();
                if matches!(state.phase, Phase::Finished { .. }) {
                    break;
                }
                let seat = if let Phase::Discard { pending } = state.phase {
                    pending.trailing_zeros() as u8
                } else {
                    state.current
                };
                let actions = legal_actions(&state, seat);
                let action = actions[(step + seed as usize) % actions.len()];
                state = apply(&state, seat, action).unwrap();
            }
            state.validate().unwrap();
            if !matches!(state.phase, Phase::Finished { .. }) {
                state = retire(&state, 0).unwrap().0;
                state.validate().unwrap();
            }
        }
    }
}
