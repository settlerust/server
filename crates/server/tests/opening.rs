mod common;
use common::*;
use server::{model::Input, reducer};
use settlers_protocol::*;
#[tokio::test]
async fn opening_is_public_replayable_and_waits_for_players_before_setup() {
    let h = Harness::new().await;
    for p in 1..4 {
        h.command(
            p,
            Command::Join {
                name: format!("Player {p}"),
                spectator: false,
            },
        )
        .await;
    }
    h.ready_all().await;
    let before = h.room().await;
    let input = Input::Command {
        identity: identity(0),
        request: request(before.version, Command::RollForOrder),
    };
    let (rolled, record) = reducer::reduce(&before, input.clone(), 2).unwrap();
    let (replayed, replay_record) = reducer::reduce(&before, input, 2).unwrap();
    assert_eq!(record, replay_record);
    assert_eq!(
        reducer::state_hash(&rolled).unwrap(),
        reducer::state_hash(&replayed).unwrap()
    );
    assert!(rolled.game.is_none());
    assert!(rolled.deadline_ms.is_none());
    let opening = rolled.opening.as_ref().unwrap();
    assert!(opening.rolls.len() >= 4);
    let final_round = opening.rolls.last().unwrap().round;
    let final_rolls: Vec<_> = opening
        .rolls
        .iter()
        .filter(|r| r.round == final_round)
        .collect();
    let best = final_rolls
        .iter()
        .map(|r| r.first + r.second)
        .max()
        .unwrap();
    assert_eq!(
        final_rolls
            .iter()
            .filter(|r| r.first + r.second == best)
            .count(),
        1
    );
    assert_eq!(
        final_rolls
            .iter()
            .find(|r| r.first + r.second == best)
            .unwrap()
            .player,
        opening.first
    );
    assert!(
        opening
            .rolls
            .iter()
            .all(|r| (1..=6).contains(&r.first) && (1..=6).contains(&r.second))
    );
    assert_eq!(
        reducer::view(&rolled, "player-1").unwrap().opening.as_ref(),
        Some(opening)
    );
    let start = Input::Command {
        identity: identity(0),
        request: request(rolled.version, Command::Start),
    };
    let (started, _) = reducer::reduce(&rolled, start, 3).unwrap();
    assert_eq!(started.game.unwrap().first, opening.first);
    assert_eq!(started.game.unwrap().current, opening.first);
    assert!(
        reducer::reduce(
            &rolled,
            Input::Command {
                identity: identity(0),
                request: request(rolled.version, Command::RollForOrder)
            },
            3
        )
        .is_err()
    );
    assert!(
        reducer::reduce(
            &before,
            Input::Command {
                identity: identity(9),
                request: request(before.version, Command::RollForOrder)
            },
            3
        )
        .is_err()
    );
}
