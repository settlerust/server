mod common;
use common::*;
use server::{
    reducer,
    service::GameService,
    storage::{Archive, Store},
};
use settlers_protocol::*;
async fn disconnect(h: &Harness, p: u8, now: u64) {
    h.service
        .presence(&h.id, identity(p).subject, format!("test-{p}"), false, now)
        .await
        .unwrap();
}
async fn resolve(
    h: &Harness,
    host: u8,
    seat: u8,
    at: u64,
    now: u64,
    resolution: DisconnectResolution,
) -> server::error::Result<Receipt> {
    h.service
        .execute(
            &h.id,
            identity(host),
            request(
                h.room().await.version,
                Command::ResolveDisconnect {
                    seat,
                    disconnected_at_ms: at,
                    resolution,
                },
            ),
            now,
        )
        .await
}
#[tokio::test]
async fn multiple_tabs_aggregate_activity_and_last_disconnect_broadcasts_once() {
    let h = Harness::new().await;
    h.start().await;
    let mut events = h.service.subscribe(&h.id, "player-0").await.unwrap();
    h.service
        .presence(&h.id, "player-1".into(), "second".into(), true, 10)
        .await
        .unwrap();
    h.service
        .activity(&h.id, "player-1".into(), "second".into(), true, 20)
        .await
        .unwrap();
    assert!(!h.room().await.members[1].idle);
    h.service
        .activity(&h.id, "player-1".into(), "test-1".into(), true, 30)
        .await
        .unwrap();
    assert!(h.room().await.members[1].idle);
    disconnect(&h, 1, 100).await;
    assert!(h.room().await.members[1].connected);
    h.service
        .presence(&h.id, "player-1".into(), "second".into(), false, 1000)
        .await
        .unwrap();
    let room = h.room().await;
    assert_eq!(room.members[1].disconnected_at_ms, Some(1000));
    let mut offline = 0;
    while let Ok(record) = events.try_recv() {
        offline += record
            .public
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    PublicEvent::Presence {
                        seat: Some(1),
                        connected: false,
                        ..
                    }
                )
            })
            .count();
    }
    assert_eq!(offline, 1);
    let history = h.service.history(&h.id, "player-0", 0, 100).await.unwrap();
    assert!(
        history
            .events
            .iter()
            .flat_map(|b| &b.events)
            .any(|e| matches!(
                e,
                PublicEvent::Presence {
                    seat: Some(1),
                    connected: false,
                    ..
                }
            ))
    );
}
#[tokio::test]
async fn host_choice_is_timed_authorized_replayable_and_revokes_former_control() {
    let h = Harness::new().await;
    h.start().await;
    disconnect(&h, 1, 1000).await;
    assert!(
        resolve(&h, 0, 1, 1000, 120999, DisconnectResolution::Retire)
            .await
            .is_err()
    );
    assert!(
        resolve(&h, 2, 1, 1000, 121000, DisconnectResolution::Retire)
            .await
            .is_err()
    );
    resolve(&h, 0, 1, 1000, 121000, DisconnectResolution::Retire)
        .await
        .unwrap();
    let former = h.service.view(&h.id, "player-1").await.unwrap();
    assert_eq!(former.game.unwrap().seat, None);
    assert_eq!(h.room().await.game.unwrap().retired, 1 << 1);
    assert!(
        h.service
            .execute(
                &h.id,
                identity(1),
                request(
                    h.room().await.version,
                    Command::Communicate {
                        to: None,
                        message: Communication {
                            intent: Intent::Chat,
                            resources: None,
                            text: Some("old controller".into())
                        }
                    }
                ),
                121001
            )
            .await
            .is_err()
    );
    h.service.flush_archive().await.unwrap();
    let replay = reducer::replay(&h.archive.read(&h.id).await.unwrap()).unwrap();
    assert_eq!(
        replay.members[1].resolution,
        Some(DisconnectResolution::Retire)
    );
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), 122000)
        .await
        .unwrap();
    assert_eq!(
        restored.room(&h.id).await.unwrap().game.unwrap().retired,
        1 << 1
    );
}
#[tokio::test]
async fn reconnect_cancels_prompt_and_fences_a_stale_choice() {
    let h = Harness::new().await;
    h.start().await;
    disconnect(&h, 1, 1000).await;
    h.service
        .presence(&h.id, "player-1".into(), "back".into(), true, 125000)
        .await
        .unwrap();
    assert!(
        resolve(&h, 0, 1, 1000, 126000, DisconnectResolution::Expert)
            .await
            .is_err()
    );
    h.service.tick(160000).await.unwrap();
    assert_eq!(h.room().await.members[1].resolution, None);
    assert_eq!(h.room().await.members[1].disconnected_at_ms, None);
}
#[tokio::test]
async fn never_connected_seats_can_be_resolved_before_starting() {
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
    for p in [0, 3] {
        h.service
            .presence(&h.id, identity(p).subject, format!("test-{p}"), true, 0)
            .await
            .unwrap();
        h.command(p, Command::SetReady { ready: true }).await;
    }
    h.service.tick(1000).await.unwrap();
    assert_eq!(h.room().await.members[1].disconnected_at_ms, Some(1000));
    assert_eq!(h.room().await.members[2].disconnected_at_ms, Some(1000));
    resolve(&h, 0, 1, 1000, 121000, DisconnectResolution::Retire)
        .await
        .unwrap();
    h.service.tick(151000).await.unwrap();
    assert_eq!(
        h.room().await.members[2].resolution,
        Some(DisconnectResolution::Expert)
    );
    h.service
        .execute(
            &h.id,
            identity(0),
            request(h.room().await.version, Command::Start),
            151001,
        )
        .await
        .unwrap();
    assert_eq!(h.room().await.game.unwrap().retired, 1 << 1);
}
#[tokio::test]
async fn expert_takeover_releases_partial_discard_selection_without_moving_cards() {
    let h = Harness::trading().await;
    let mut room = h.room().await;
    room.members[1].connected = false;
    room.members[1].disconnected_at_ms = Some(1000);
    room.game.as_mut().unwrap().phase = Phase::Discard { pending: 1 << 1 };
    room.discard = Some(server::model::DiscardRound {
        id: 1,
        required: [0, 5, 0, 0],
        locked: [
            Resources::default(),
            Resources([2, 0, 0, 0, 0]),
            Resources::default(),
            Resources::default(),
        ],
        drafts: [
            Resources::default(),
            Resources([0, 1, 0, 0, 0]),
            Resources::default(),
            Resources::default(),
        ],
        deadline_ms: 0,
        turn_remaining_ms: 0,
    });
    let hand = room.game.unwrap().players[1].resources;
    let (next, _) = reducer::reduce(
        &room,
        server::model::Input::Command {
            identity: identity(0),
            request: request(
                room.version,
                Command::ResolveDisconnect {
                    seat: 1,
                    disconnected_at_ms: 1000,
                    resolution: DisconnectResolution::Expert,
                },
            ),
        },
        121000,
    )
    .unwrap();
    let round = next.discard.unwrap();
    assert_eq!(round.required[1], 5);
    assert_eq!(round.locked[1], Resources::default());
    assert_eq!(round.drafts[1], Resources::default());
    assert_eq!(next.game.unwrap().players[1].resources, hand);
}
#[tokio::test]
async fn automatic_expert_takeover_occurs_after_two_minutes_plus_thirty_seconds_without_host() {
    let h = Harness::new().await;
    h.start().await;
    let seat = h.room().await.game.unwrap().current;
    disconnect(&h, seat, 1000).await;
    h.service.tick(150999).await.unwrap();
    assert_eq!(h.room().await.members[seat as usize].resolution, None);
    h.service.tick(151000).await.unwrap();
    assert_eq!(
        h.room().await.members[seat as usize].resolution,
        Some(DisconnectResolution::Expert)
    );
    let version = h.room().await.version;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if h.room().await.version > version {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        h.store
            .pending(&h.id)
            .await
            .unwrap()
            .iter()
            .any(|record| matches!(record.input, server::model::Input::ComputerCommand { .. }))
    );
    assert!(
        h.service
            .view(&h.id, &identity(seat).subject)
            .await
            .unwrap()
            .game
            .unwrap()
            .own
            .is_none()
    );
}

#[tokio::test]
async fn host_can_choose_each_builtin_strength_and_former_player_loses_control() {
    for resolution in [
        DisconnectResolution::Easy,
        DisconnectResolution::Medium,
        DisconnectResolution::Hard,
        DisconnectResolution::Expert,
    ] {
        let h = Harness::new().await;
        h.start().await;
        disconnect(&h, 1, 1000).await;
        resolve(&h, 0, 1, 1000, 121000, resolution).await.unwrap();
        let room = h.room().await;
        assert_eq!(room.members[1].policy(), resolution.computer_policy());
        assert_eq!(
            h.service
                .view(&h.id, "player-1")
                .await
                .unwrap()
                .game
                .unwrap()
                .seat,
            None
        );
        let view = h.service.view(&h.id, "player-0").await.unwrap();
        assert!(view.members[1].connected);
        h.service.flush_archive().await.unwrap();
        let replay = reducer::replay(&h.archive.read(&h.id).await.unwrap()).unwrap();
        assert_eq!(replay.members[1].resolution, Some(resolution));
    }
}
