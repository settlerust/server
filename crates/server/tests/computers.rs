mod common;
use common::*;
use server::{
    model::Input,
    reducer,
    service::GameService,
    storage::{Archive, Store},
};
use settlers_protocol::*;

#[tokio::test]
async fn every_builtin_starts_plays_and_survives_replay_without_a_connection() {
    for policy in [
        ComputerPolicy::Easy,
        ComputerPolicy::Medium,
        ComputerPolicy::Hard,
        ComputerPolicy::Expert,
    ] {
        let h = Harness::new().await;
        let view = h
            .service
            .create(
                identity(9),
                CreateGame {
                    name: "Server computers".into(),
                    research: false,
                    host_as_player: false,
                    player_name: None,
                    seed: None,
                    settings: Settings {
                        players: 2,
                        turn_seconds: 0,
                        ..Settings::default()
                    },
                },
                0,
            )
            .await
            .unwrap();
        let update = AdmissionUpdate {
            revision: 1,
            players: vec!["computer-0".into(), "computer-1".into()],
            computers: (0..2)
                .map(|i| ComputerSeat {
                    subject: format!("computer-{i}"),
                    name: format!("AI {i}"),
                    policy,
                })
                .collect(),
        };
        h.service
            .admission(&view.id, identity(9), update.clone(), 1)
            .await
            .unwrap();
        let room = h.service.room(&view.id).await.unwrap();
        let view = h
            .service
            .view(&view.id, &identity(9).subject)
            .await
            .unwrap();
        assert!(
            view.members
                .iter()
                .filter(|m| m.seat.is_some())
                .all(|m| m.connected && m.ready && m.computer_policy == Some(policy))
        );
        assert!(
            room.members
                .iter()
                .all(|m| !m.connected && m.disconnected_at_ms.is_none())
        );
        assert!(
            h.service
                .execute(
                    &view.id,
                    server::auth::Identity {
                        subject: "computer-0".into(),
                        name: "Imposter".into()
                    },
                    request(room.version, Command::SetReady { ready: false }),
                    1
                )
                .await
                .is_err()
        );
        h.service
            .execute(
                &view.id,
                identity(9),
                request(room.version, Command::Start),
                2,
            )
            .await
            .unwrap();
        let before = h.service.room(&view.id).await.unwrap().version;
        h.service.tick(3).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if h.service.room(&view.id).await.unwrap().version > before {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let room = h.service.room(&view.id).await.unwrap();
        assert!(matches!(room.game.unwrap().phase, Phase::SetupRoad { .. }));
        assert!(room.members.iter().all(|m| m.disconnected_at_ms.is_none()));
        assert!(
            h.store
                .pending(&view.id)
                .await
                .unwrap()
                .iter()
                .any(|r| matches!(r.input, Input::ComputerCommand { .. }))
        );
        h.service.flush_archive().await.unwrap();
        let replay = reducer::replay(&h.archive.read(&view.id).await.unwrap()).unwrap();
        assert_eq!(
            reducer::state_hash(&replay).unwrap(),
            reducer::state_hash(&room).unwrap()
        );
        let restored = GameService::restore(h.store.clone(), h.archive.clone(), 4)
            .await
            .unwrap();
        assert_eq!(
            restored.room(&view.id).await.unwrap().admission,
            Some(update)
        );
        assert_eq!(
            restored
                .view(&view.id, "computer-0")
                .await
                .unwrap()
                .game
                .unwrap()
                .seat,
            None
        );
    }
}

#[tokio::test]
async fn builtin_admission_is_fenced_and_cannot_take_over_a_human() {
    let h = Harness::new().await;
    h.command(
        1,
        Command::Join {
            name: "Friend".into(),
            spectator: false,
        },
    )
    .await;
    let mut update = AdmissionUpdate {
        revision: 1,
        players: vec![identity(0).subject, identity(1).subject],
        computers: vec![ComputerSeat {
            subject: identity(1).subject,
            name: "AI".into(),
            policy: ComputerPolicy::Easy,
        }],
    };
    assert!(
        h.service
            .admission(&h.id, identity(0), update.clone(), 2)
            .await
            .is_err()
    );
    update.players[1] = "computer".into();
    update.computers[0].subject = "computer".into();
    assert!(
        h.service
            .admission(&h.id, identity(1), update.clone(), 2)
            .await
            .is_err()
    );
    h.service
        .admission(&h.id, identity(0), update.clone(), 2)
        .await
        .unwrap();
    let version = h.room().await.version;
    h.service
        .admission(&h.id, identity(0), update.clone(), 3)
        .await
        .unwrap();
    assert_eq!(h.room().await.version, version);
    update.computers[0].policy = ComputerPolicy::Hard;
    assert!(
        h.service
            .admission(&h.id, identity(0), update.clone(), 4)
            .await
            .is_err()
    );
    update.revision += 1;
    h.service
        .admission(&h.id, identity(0), update, 5)
        .await
        .unwrap();
    assert_eq!(
        h.room().await.member("computer").unwrap().policy(),
        Some(ComputerPolicy::Hard)
    );
}
