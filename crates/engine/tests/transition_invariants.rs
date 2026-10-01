mod common;

use common::conservation;
use settlers_engine::*;

#[test]
fn every_advertised_action_is_valid_and_preserves_inventory_across_seeded_games() {
    for players in 2..=4 {
        for deterministic in [false, true] {
            for seed in 0..4 {
                let mut state = State::new(
                    Settings {
                        players,
                        deterministic,
                        ..Settings::default()
                    },
                    [seed; 32],
                )
                .unwrap();
                let mut chooser = Random::new([seed + 1; 32]);
                for _ in 0..120 {
                    if matches!(state.phase, Phase::Finished { .. }) {
                        break;
                    }
                    let player = if let Phase::Discard { pending } = state.phase {
                        pending.trailing_zeros() as u8
                    } else {
                        state.current
                    };
                    let actions = legal_actions(&state, player);
                    assert!(!actions.is_empty(), "no action in {:?}", state.phase);
                    for &action in &actions {
                        let next = apply(&state, player, action).unwrap_or_else(|error| {
                            panic!("advertised {action:?} failed in {:?}: {error}", state.phase)
                        });
                        conservation(&next);
                        let (recorded, _) = apply_recorded(&state, player, action).unwrap();
                        assert_eq!(recorded, next);
                    }
                    let action = actions[chooser.below(actions.len() as u32) as usize];
                    state = apply(&state, player, action).unwrap();
                }
            }
        }
    }
}

#[test]
fn all_out_of_range_seats_and_board_locations_are_rejected() {
    let mut state = State::new(Settings::default(), [1; 32]).unwrap();
    while state.turn == 0 {
        state = timeout(&state).unwrap().0;
    }
    state.phase = Phase::Main;
    for player in state.settings.players..=u8::MAX {
        assert_eq!(
            apply(&state, player, Action::EndTurn),
            Err(RuleError::InvalidPlayer)
        );
        assert!(legal_actions(&state, player).is_empty());
        assert!(observe(&state, Some(player)).own.is_none());
    }
    for vertex in 54..=u8::MAX {
        for action in [
            Action::BuildSettlement { vertex },
            Action::BuildCity { vertex },
        ] {
            assert_eq!(
                apply(&state, state.current, action),
                Err(RuleError::IllegalPlacement)
            );
        }
    }
    for edge in 72..=u8::MAX {
        assert_eq!(
            apply(&state, state.current, Action::BuildRoad { edge }),
            Err(RuleError::IllegalPlacement)
        );
    }
    state.phase = Phase::Robber {
        resume: Resume::Main,
    };
    for hex in 19..=u8::MAX {
        assert_eq!(
            apply(
                &state,
                state.current,
                Action::MoveRobber { hex, victim: None }
            ),
            Err(RuleError::InvalidLocation)
        );
    }
}
