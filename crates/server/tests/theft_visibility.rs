mod common;
use common::*;
use server::{model::Input, reducer};
use settlers_engine::{Action, Phase};
use settlers_protocol::*;

#[test]
fn theft_resource_is_private_and_old_archives_still_replay() {
    let (mut room, first) = reducer::create(
        "theft-table".into(),
        CreateGame {
            research: false,
            name: "Theft visibility".into(),
            settings: Settings::default(),
            host_as_player: true,
            player_name: Some("Host display name".into()),
        },
        identity(0),
        [8; 32],
        0,
    )
    .unwrap();
    assert_eq!(room.members[0].identity.name, "Host display name");
    let mut records = vec![first];
    for p in 1..=4 {
        let (next, record) = reducer::reduce(
            &room,
            Input::Command {
                identity: identity(p),
                request: request(
                    room.version,
                    Command::Join {
                        name: format!("Player {p}"),
                        spectator: p == 4,
                    },
                ),
            },
            1,
        )
        .unwrap();
        room = next;
        records.push(record);
    }
    let (next, record) = reducer::reduce(
        &room,
        Input::Command {
            identity: identity(0),
            request: request(room.version, Command::Start),
        },
        2,
    )
    .unwrap();
    room = next;
    records.push(record);
    let mut found = None;
    for _ in 0..1200 {
        let game = room.game.unwrap();
        let actor = if let Phase::Discard { pending } = game.phase {
            pending.trailing_zeros() as u8
        } else {
            game.current
        };
        let observation = settlers_engine::observe(&game, Some(actor));
        let action = observation
            .legal_actions
            .iter()
            .find(|a| {
                matches!(
                    a,
                    Action::MoveRobber {
                        victim: Some(_),
                        ..
                    }
                )
            })
            .or_else(|| {
                observation
                    .legal_actions
                    .iter()
                    .find(|a| matches!(a, Action::EndTurn))
            })
            .or_else(|| observation.legal_actions.first())
            .copied()
            .unwrap();
        let (next, record) = reducer::reduce(
            &room,
            Input::Command {
                identity: identity(actor),
                request: request(room.version, Command::Act { action }),
            },
            3,
        )
        .unwrap();
        room = next;
        found = record.public.iter().find_map(|e| {
            if let PublicEvent::TheftDetails {
                player,
                victim,
                resource,
            } = e
            {
                Some((*player, *victim, *resource))
            } else {
                None
            }
        });
        records.push(record);
        if found.is_some() {
            break;
        }
    }
    let (thief, victim, resource) = found.expect("fixture must reach a theft");
    for p in 0..=4 {
        let batch = reducer::batch(&room, &identity(p).subject, records.last().unwrap()).unwrap();
        let details: Vec<_> = batch
            .events
            .iter()
            .filter(|e| matches!(e, PublicEvent::TheftDetails { .. }))
            .collect();
        if p == thief || p == victim {
            assert_eq!(
                details,
                vec![&PublicEvent::TheftDetails {
                    player: thief,
                    victim,
                    resource
                }]
            );
        } else {
            assert!(details.is_empty());
            assert!(
                !serde_json::to_string(&batch)
                    .unwrap()
                    .contains("theft_details")
            );
        }
        assert!(batch.events.iter().any(|e| matches!(e, PublicEvent::Rule { event: settlers_engine::Event::Stole { player, victim: v, count: 1 } } if *player == thief && *v == victim)));
    }
    let expected = reducer::state_hash(&room).unwrap();
    assert_eq!(
        reducer::state_hash(&reducer::replay(&records).unwrap()).unwrap(),
        expected
    );
    let legacy: Vec<_> = records
        .into_iter()
        .map(|mut r| {
            r.engine_version = "base-v2".into();
            r.public
                .retain(|e| !matches!(e, PublicEvent::TheftDetails { .. }));
            r
        })
        .collect();
    assert_eq!(
        reducer::state_hash(&reducer::replay(&legacy).unwrap()).unwrap(),
        expected
    );
}
