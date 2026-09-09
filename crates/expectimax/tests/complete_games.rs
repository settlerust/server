mod common;
use settlers_engine::*;
use settlers_expectimax::{Config, analyze, leaf};
use settlers_protocol::Command;

#[test]
fn reference_player_finishes_two_three_and_four_player_games() {
    for (players, self_play) in [(2, false), (3, false), (4, false), (4, true)] {
        let (mut state, mut history) = common::fixture(players);
        let mut offered = [None; 4];
        for _ in 0..5000 {
            if matches!(state.phase, Phase::Finished { .. }) {
                break;
            }
            let actor = match state.phase {
                Phase::Discard { pending } => pending.trailing_zeros() as u8,
                _ => state.current,
            };
            let action = if actor == 0 || self_play {
                let report = analyze(
                    &common::view(&state, actor, history.len() as u64),
                    &history,
                    Config {
                        samples: 2,
                        node_budget: 16,
                        ..Config::default()
                    },
                    offered[actor as usize],
                )
                .unwrap_or_else(|e| {
                    panic!(
                        "{players} players, turn {}, phase {:?}: {e}",
                        state.turn, state.phase
                    )
                });
                let command = report.candidates[0].command.as_ref().unwrap();
                match command {
                    Command::Act { action } => *action,
                    Command::OfferTrade {
                        to, give, receive, ..
                    } => {
                        offered[actor as usize] = Some(state.turn);
                        let trade = Action::Trade {
                            other: to[0],
                            give: *give,
                            receive: *receive,
                        };
                        if apply(&state, actor, trade).is_ok() {
                            trade
                        } else {
                            continue;
                        }
                    }
                    _ => panic!("unexpected command"),
                }
            } else {
                legal_actions(&state, actor)
                    .into_iter()
                    .filter(|a| !matches!(a, Action::RelocateSettlement { .. }))
                    .max_by(|a, b| {
                        leaf::action_score(&state, actor, *a)
                            .total_cmp(&leaf::action_score(&state, actor, *b))
                    })
                    .unwrap()
            };
            let (next, events) = apply_recorded(&state, actor, action).unwrap();
            state = next;
            common::push(&mut history, events);
            for r in 0..5 {
                assert_eq!(
                    state.bank.0[r] as u16
                        + state
                            .players
                            .iter()
                            .map(|p| p.resources.0[r] as u16)
                            .sum::<u16>(),
                    19
                );
            }
        }
        assert!(
            matches!(state.phase, Phase::Finished { .. }),
            "{players} players stopped at turn {}",
            state.turn
        );
        eprintln!(
            "{players} players, self_play={self_play}: finished turn {}",
            state.turn
        );
    }
}
