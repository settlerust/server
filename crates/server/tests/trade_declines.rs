mod common;
use common::*;
use server::service::GameService;
use server::{reducer, storage::Archive};
use settlers_protocol::*;

#[tokio::test]
async fn explicit_passes_survive_restore_and_the_last_pass_closes_the_offer() {
    let h = Harness::trading().await;
    h.command(
        0,
        Command::OfferTrade {
            to: vec![1, 2],
            give: Resources([1, 0, 0, 0, 0]),
            receive: Resources([0, 1, 0, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    let offer_id = h.room().await.offers.last().unwrap().public.id.clone();
    h.command(
        1,
        Command::RejectTrade {
            offer_id: offer_id.clone(),
        },
    )
    .await;
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), 2)
        .await
        .unwrap();
    let sender = restored.view(&h.id, &identity(0).subject).await.unwrap();
    assert_eq!(sender.offers[0].declined, vec![1]);
    assert_eq!(
        restored
            .view(&h.id, &identity(2).subject)
            .await
            .unwrap()
            .offers[0]
            .declined,
        vec![1]
    );
    assert!(
        restored
            .view(&h.id, &identity(1).subject)
            .await
            .unwrap()
            .offers
            .is_empty()
    );
    let version = sender.version;
    restored
        .execute(
            &h.id,
            identity(2),
            request(
                version,
                Command::RejectTrade {
                    offer_id: offer_id.clone(),
                },
            ),
            3,
        )
        .await
        .unwrap();
    for player in 0..3 {
        assert!(
            restored
                .view(&h.id, &identity(player).subject)
                .await
                .unwrap()
                .offers
                .is_empty()
        );
    }
    let events = restored
        .history(&h.id, &identity(0).subject, version, 100)
        .await
        .unwrap()
        .events;
    assert!(events[0].events.iter().any(|e| matches!(
        e,
        PublicEvent::TradeClosed {
            reason: TradeCloseReason::Declined,
            ..
        }
    )));
    let room = restored.room(&h.id).await.unwrap();
    assert_eq!(
        room.offers.last().unwrap().public.status,
        OfferStatus::Cancelled
    );
    assert_eq!(
        restored
            .execute(
                &h.id,
                identity(2),
                request(room.version, Command::AcceptTrade { offer_id }),
                4
            )
            .await
            .unwrap_err()
            .0
            .code,
        ErrorCode::TradeUnavailable
    );
}

#[tokio::test]
async fn automatic_filters_do_not_disclose_private_preferences() {
    let h = Harness::trading().await;
    h.command(
        1,
        Command::SetFilter {
            player: 0,
            filter: Filter {
                auto_reject_trades: true,
                ..Filter::default()
            },
        },
    )
    .await;
    h.command(
        0,
        Command::OfferTrade {
            to: vec![1, 2],
            give: Resources([1, 0, 0, 0, 0]),
            receive: Resources([0, 1, 0, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    let sender = h.service.view(&h.id, &identity(0).subject).await.unwrap();
    assert!(sender.offers[0].declined.is_empty());
    assert!(!sender.filters.unwrap()[1].auto_reject_trades);
    h.command(
        2,
        Command::RejectTrade {
            offer_id: sender.offers[0].id.clone(),
        },
    )
    .await;
    assert!(
        h.service
            .view(&h.id, &identity(0).subject)
            .await
            .unwrap()
            .offers
            .is_empty()
    );
    h.command(
        0,
        Command::OfferTrade {
            to: vec![1],
            give: Resources([1, 0, 0, 0, 0]),
            receive: Resources([0, 1, 0, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    assert!(
        h.service
            .view(&h.id, &identity(0).subject)
            .await
            .unwrap()
            .offers
            .is_empty()
    );
}

#[tokio::test]
async fn archived_declines_replay_with_current_transition_version() {
    let h = Harness::new().await;
    h.start().await;
    loop {
        let game = h.room().await.game.unwrap();
        if game.phase == Phase::Main {
            break;
        }
        let player = if let Phase::Discard { pending } = game.phase {
            pending.trailing_zeros() as u8
        } else {
            game.current
        };
        let action = settlers_engine::legal_actions(&game, player)[0];
        h.command(player, Command::Act { action }).await;
    }
    let game = h.room().await.game.unwrap();
    let player = game.current;
    let to: Vec<_> = (0..game.settings.players)
        .filter(|p| *p != player)
        .collect();
    let resource = game.players[player as usize]
        .resources
        .0
        .iter()
        .position(|count| *count > 0)
        .unwrap();
    let mut give = Resources([0; 5]);
    give.0[resource] = 1;
    let mut receive = Resources([0; 5]);
    receive.0[(resource + 1) % 5] = 1;
    h.command(
        player,
        Command::OfferTrade {
            to: to.clone(),
            give,
            receive,
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    let offer_id = h.room().await.offers.last().unwrap().public.id.clone();
    for recipient in to {
        h.command(
            recipient,
            Command::RejectTrade {
                offer_id: offer_id.clone(),
            },
        )
        .await;
    }
    let final_room = h.room().await;
    h.service.flush_archive().await.unwrap();
    let records = h.archive.read(&h.id).await.unwrap();
    assert_eq!(
        reducer::state_hash(&reducer::replay(&records).unwrap()).unwrap(),
        reducer::state_hash(&final_room).unwrap()
    );
}
