mod common;
use common::*;
use server::storage::Store;
use server::{
    model::{Input, Room},
    reducer,
};
use settlers_engine::{Action, Phase, Resources};
use settlers_protocol::*;

fn command(room: &Room, p: u8, command: Command, now: u64) -> (Room, server::model::Record) {
    reducer::reduce(
        room,
        Input::Command {
            identity: identity(p),
            request: request(room.version, command),
        },
        now,
    )
    .unwrap()
}
async fn seven(seconds: u32) -> Room {
    let h = Harness::trading().await;
    let mut room = h.room().await;
    room.settings.turn_seconds = seconds;
    room.deadline_ms = Some(1000 + u64::from(seconds) * 1000);
    let mut game = room.game.unwrap();
    game.settings.turn_seconds = seconds;
    for _ in 0..100 {
        game.phase = Phase::Roll;
        let (next, events) = settlers_engine::apply_recorded(&game, 0, Action::Roll).unwrap();
        if events.iter().any(|e| matches!(e, settlers_engine::Event::Rolled {first, second, ..} if first+second == 7)) {
            room.game = Some(game);
            return command(&room, 0, Command::Act {action: Action::Roll}, 1000).0;
        }
        game.random = next.random;
    }
    panic!("fixture must find a seven");
}
#[tokio::test]
async fn choices_are_private_editable_then_locked_in_stages_without_extending_the_deadline() {
    let mut room = seven(120).await;
    let id = room.discard.as_ref().unwrap().id;
    assert_eq!(room.deadline_ms, Some(16000));
    let (next, private) = command(
        &room,
        0,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([1, 0, 0, 0, 0]),
        },
        2000,
    );
    assert!(private.public.is_empty());
    room = command(
        &next,
        0,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([0, 2, 0, 0, 0]),
        },
        3000,
    )
    .0;
    assert_eq!(
        reducer::view(&room, "player-0")
            .unwrap()
            .discard
            .unwrap()
            .draft,
        Resources([0, 2, 0, 0, 0])
    );
    assert_eq!(
        reducer::view(&room, "player-1")
            .unwrap()
            .discard
            .unwrap()
            .draft,
        Resources::default()
    );
    let observer = command(
        &room,
        9,
        Command::Join {
            name: "Observer".into(),
            spectator: true,
        },
        3001,
    )
    .0;
    assert!(
        reducer::view(&observer, "player-9")
            .unwrap()
            .discard
            .is_none()
    );
    room = command(&room, 0, Command::ConfirmDiscard { discard_id: id }, 4000).0;
    assert_eq!(
        room.discard.as_ref().unwrap().locked[0],
        Resources([0, 2, 0, 0, 0])
    );
    assert!(
        reducer::reduce(
            &room,
            Input::Command {
                identity: identity(0),
                request: request(
                    room.version,
                    Command::SelectDiscard {
                        discard_id: id,
                        resources: Resources([0, 1, 0, 0, 0])
                    }
                )
            },
            4500
        )
        .is_err()
    );
    assert!(
        reducer::reduce(
            &room,
            Input::Command {
                identity: identity(0),
                request: request(
                    room.version,
                    Command::Act {
                        action: Action::Discard {
                            resources: Resources([2, 0, 1, 1, 1])
                        }
                    }
                )
            },
            4500
        )
        .is_err()
    );
    room = command(
        &room,
        0,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([2, 0, 1, 0, 0]),
        },
        5000,
    )
    .0;
    let (next, resolved) = command(&room, 0, Command::ConfirmDiscard { discard_id: id }, 6000);
    room = next;
    assert_eq!(
        room.game.unwrap().players[0].resources,
        Resources([0, 0, 1, 2, 2])
    );
    assert_eq!(room.deadline_ms, Some(16000));
    assert_eq!(
        resolved
            .public
            .iter()
            .filter(|e| matches!(
                e,
                PublicEvent::Rule {
                    event: settlers_engine::Event::Discarded {
                        player: 0,
                        count: 5
                    }
                }
            ))
            .count(),
        1
    );
    let (disconnected, _) = reducer::reduce(
        &room,
        Input::Presence {
            subject: "player-1".into(),
            connected: true,
        },
        7000,
    )
    .unwrap();
    let (disconnected, _) = reducer::reduce(
        &disconnected,
        Input::Presence {
            subject: "player-1".into(),
            connected: false,
        },
        8000,
    )
    .unwrap();
    assert_eq!(disconnected.deadline_ms, Some(16000));
}
#[tokio::test]
async fn timeout_preserves_locked_and_selected_cards_fills_only_the_remainder_and_keeps_robber_choice()
 {
    let mut room = seven(40).await;
    let id = room.discard.as_ref().unwrap().id;
    assert_eq!(room.deadline_ms, Some(11000));
    room = command(
        &room,
        0,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([0, 0, 0, 0, 1]),
        },
        2000,
    )
    .0;
    room = command(&room, 0, Command::ConfirmDiscard { discard_id: id }, 2001).0;
    room = command(
        &room,
        0,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([0, 0, 0, 2, 0]),
        },
        3000,
    )
    .0;
    // Persisted private staging survives a process restart / JSON storage round trip.
    let restored: Room = serde_json::from_slice(&serde_json::to_vec(&room).unwrap()).unwrap();
    let (finished, record) = reducer::reduce(&restored, Input::Timeout, 11000).unwrap();
    assert!(matches!(finished.game.unwrap().phase, Phase::Robber { .. }));
    assert_eq!(
        finished.game.unwrap().players[0].resources,
        Resources([1, 1, 2, 0, 1])
    );
    assert!(
        finished
            .game
            .unwrap()
            .players
            .iter()
            .all(|p| p.resources.total() == 5)
    );
    assert_eq!(finished.deadline_ms, Some(51000));
    assert!(finished.discard.is_none());
    let (same, same_record) = reducer::reduce(&room, Input::Timeout, 11000).unwrap();
    assert_eq!(record, same_record);
    assert_eq!(
        reducer::state_hash(&same).unwrap(),
        reducer::state_hash(&finished).unwrap()
    );
    assert!(
        reducer::reduce(
            &room,
            Input::Command {
                identity: identity(0),
                request: request(room.version, Command::ConfirmDiscard { discard_id: id })
            },
            11000
        )
        .is_err()
    );
}
#[tokio::test]
async fn independent_players_can_select_against_an_older_version_but_never_a_different_round() {
    let room = seven(120).await;
    let id = room.discard.as_ref().unwrap().id;
    let stale = request(
        room.version,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([1, 0, 0, 0, 0]),
        },
    );
    let (changed, _) = command(
        &room,
        0,
        Command::SelectDiscard {
            discard_id: id,
            resources: Resources([0, 1, 0, 0, 0]),
        },
        2000,
    );
    let input = Input::Command {
        identity: identity(1),
        request: stale.clone(),
    };
    let (selected, _) = reducer::reduce(&changed, input, 2001).unwrap();
    assert!(
        reducer::duplicate(&selected, &identity(1), &stale)
            .unwrap()
            .unwrap()
            .duplicate
    );
    assert!(
        reducer::reduce(
            &selected,
            Input::Command {
                identity: identity(1),
                request: request(
                    selected.version,
                    Command::SelectDiscard {
                        discard_id: id + 1,
                        resources: Resources::default()
                    }
                )
            },
            2002
        )
        .is_err()
    );
}
#[tokio::test]
async fn staged_discard_archive_replays_with_the_same_private_state_and_deadline() {
    let h = Harness::new().await;
    h.command(
        0,
        Command::Configure {
            settings: Settings {
                discard_limit: 1,
                ..h.room().await.settings
            },
        },
    )
    .await;
    h.start().await;
    for _ in 0..200 {
        let room = h.room().await;
        let game = room.game.unwrap();
        if let Some(d) = &room.discard {
            let Phase::Discard { pending } = game.phase else {
                panic!()
            };
            let p = pending.trailing_zeros() as u8;
            let resources = settlers_engine::automatic_discard(game.players[p as usize].resources);
            h.command(
                p,
                Command::SelectDiscard {
                    discard_id: d.id,
                    resources,
                },
            )
            .await;
            h.command(p, Command::ConfirmDiscard { discard_id: d.id })
                .await;
            h.service.tick(d.deadline_ms).await.unwrap();
            let records = h.store.pending(&h.id).await.unwrap();
            let replayed = reducer::replay(&records).unwrap();
            assert_eq!(
                reducer::state_hash(&replayed).unwrap(),
                reducer::state_hash(&h.room().await).unwrap()
            );
            return;
        }
        let action = match game.phase {
            Phase::Roll => Action::Roll,
            Phase::Main => Action::EndTurn,
            _ => settlers_engine::legal_actions(&game, game.current)[0],
        };
        h.command(game.current, Command::Act { action }).await;
    }
    panic!("expected a discard round");
}
