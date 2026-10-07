mod common;
use common::*;
use server::{reducer, storage::Store};
use settlers_protocol::*;

#[tokio::test]
async fn all_players_must_confirm_over_live_sockets_and_any_player_can_start() {
    let h = Harness::new().await;
    for p in 1..4 {
        h.command(
            p,
            Command::JoinAgent {
                name: format!("Agent {p}"),
            },
        )
        .await;
    }
    let denied = h
        .service
        .execute(
            &h.id,
            identity(0),
            request(h.room().await.version, Command::SetReady { ready: true }),
            1,
        )
        .await;
    assert_eq!(denied.unwrap_err().0.code, ErrorCode::Conflict);
    for p in 0..4 {
        h.service
            .presence(&h.id, identity(p).subject, format!("socket-{p}"), true, 1)
            .await
            .unwrap();
        if p < 3 {
            h.command(p, Command::SetReady { ready: true }).await;
        }
    }
    for command in [Command::Start, Command::RollForOrder] {
        let denied = h
            .service
            .execute(
                &h.id,
                identity(1),
                request(h.room().await.version, command),
                2,
            )
            .await;
        assert_eq!(denied.unwrap_err().0.code, ErrorCode::Conflict);
    }
    h.command(3, Command::SetReady { ready: true }).await;
    h.command(1, Command::RollForOrder).await;
    // An extra tab keeps a player connected when their first tab closes.
    h.service
        .presence(&h.id, identity(0).subject, "extra-tab".into(), true, 2)
        .await
        .unwrap();
    h.service
        .presence(&h.id, identity(0).subject, "socket-0".into(), false, 3)
        .await
        .unwrap();
    assert!(h.room().await.members[0].ready);
    h.service
        .presence(&h.id, identity(0).subject, "extra-tab".into(), false, 4)
        .await
        .unwrap();
    assert!(h.room().await.members[0].ready);
    // Opening rolls have already committed consent; readiness cannot be toggled here.
    let denied = h
        .service
        .execute(
            &h.id,
            identity(1),
            request(h.room().await.version, Command::SetReady { ready: false }),
            5,
        )
        .await;
    assert_eq!(denied.unwrap_err().0.code, ErrorCode::Conflict);
    h.command(2, Command::Start).await;
    assert!(h.room().await.game.is_some());
    let records = h.store.pending(&h.id).await.unwrap();
    assert_eq!(
        reducer::state_hash(&reducer::replay(&records).unwrap()).unwrap(),
        reducer::state_hash(&h.room().await).unwrap()
    );
}

#[tokio::test]
async fn changing_settings_clears_consent_and_spectators_cannot_mark_ready() {
    let h = Harness::new().await;
    h.service
        .presence(&h.id, identity(0).subject, "host-tab".into(), true, 0)
        .await
        .unwrap();
    h.command(0, Command::SetReady { ready: true }).await;
    h.command(
        0,
        Command::Configure {
            settings: Settings::default(),
        },
    )
    .await;
    assert!(!h.room().await.members[0].ready);
    h.command(
        9,
        Command::Join {
            name: "Observer".into(),
            spectator: true,
        },
    )
    .await;
    let denied = h
        .service
        .execute(
            &h.id,
            identity(9),
            request(h.room().await.version, Command::SetReady { ready: true }),
            1,
        )
        .await;
    assert_eq!(denied.unwrap_err().0.code, ErrorCode::Forbidden);
}

#[tokio::test]
async fn disconnects_clear_readiness_only_before_the_opening_rolls() {
    let h = Harness::new().await;
    h.service
        .presence(&h.id, identity(0).subject, "host-tab".into(), true, 0)
        .await
        .unwrap();
    h.command(0, Command::SetReady { ready: true }).await;
    h.service
        .presence(&h.id, identity(0).subject, "host-tab".into(), false, 1)
        .await
        .unwrap();
    assert!(!h.room().await.members[0].ready);
    h.service
        .presence(&h.id, identity(0).subject, "host-tab".into(), true, 2)
        .await
        .unwrap();
    assert!(!h.room().await.members[0].ready);
}

#[tokio::test]
async fn opening_confirmation_is_independent_of_later_ready_flags() {
    let h = Harness::new().await;
    for player in 1..4 {
        h.command(
            player,
            Command::Join {
                name: format!("Player {player}"),
                spectator: false,
            },
        )
        .await;
    }
    h.ready_all().await;
    h.command(0, Command::RollForOrder).await;
    let mut room = h.room().await;
    room.members[0].ready = false;
    room.members[0].connected = false;
    let (started, _) = reducer::reduce(
        &room,
        server::model::Input::Command {
            identity: identity(1),
            request: request(room.version, Command::Start),
        },
        3,
    )
    .unwrap();
    assert!(started.game.is_some());
}
