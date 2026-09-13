mod common;
use common::*;
use settlers_protocol::*;
#[tokio::test]
async fn simultaneous_acceptances_commit_first_and_decline_second_with_winner() {
    let h = Harness::trading().await;
    let offered = h
        .command(
            0,
            Command::OfferTrade {
                to: vec![1, 2],
                give: Resources([1, 0, 0, 0, 0]),
                receive: Resources([0, 1, 0, 0, 0]),
                expires_in_ms: None,
                message: Some(Communication {
                    intent: Intent::BuildRoad,
                    resources: None,
                    text: Some("You can build a road with this".into()),
                }),
            },
        )
        .await;
    let offer = h.room().await.offers.last().unwrap().public.id.clone();
    let version = offered.version;
    let a = request(
        version,
        Command::AcceptTrade {
            offer_id: offer.clone(),
        },
    );
    let b = request(version, Command::AcceptTrade { offer_id: offer });
    let (first, second) = tokio::join!(
        h.service.execute(&h.id, identity(1), a, 2),
        h.service.execute(&h.id, identity(2), b, 2)
    );
    assert!(first.is_ok());
    let error = second.unwrap_err().0;
    assert_eq!(error.code, ErrorCode::TradeTaken);
    assert_eq!(error.winner, Some(1));
    let game = h.room().await.game.unwrap();
    assert_eq!(game.players[0].resources, Resources([1, 3, 2, 2, 2]));
    assert_eq!(game.players[1].resources, Resources([3, 1, 2, 2, 2]));
    assert_eq!(game.players[2].resources, Resources([2; 5]));
}
#[tokio::test]
async fn resource_changes_invalidate_unfunded_offers_atomically() {
    let h = Harness::trading().await;
    h.command(
        0,
        Command::OfferTrade {
            to: vec![1, 2],
            give: Resources([2, 0, 0, 0, 0]),
            receive: Resources([0, 1, 0, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    let first = h.room().await.offers.last().unwrap().public.id.clone();
    h.command(
        0,
        Command::OfferTrade {
            to: vec![1, 2],
            give: Resources([2, 0, 0, 0, 0]),
            receive: Resources([0, 1, 0, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    let second = h.room().await.offers.last().unwrap().public.id.clone();
    h.command(1, Command::AcceptTrade { offer_id: first }).await;
    let before = h.room().await;
    let result = h
        .service
        .execute(
            &h.id,
            identity(2),
            request(before.version, Command::AcceptTrade { offer_id: second }),
            2,
        )
        .await;
    assert!(result.is_err());
    assert_eq!(h.room().await.version, before.version);
    assert_eq!(h.room().await.game, before.game);
}
#[tokio::test]
async fn shadow_mute_is_private_and_filters_live_and_history_messages() {
    let h = Harness::trading().await;
    h.command(
        1,
        Command::SetFilter {
            player: 0,
            filter: Filter {
                shadow_mute: true,
                auto_reject_trades: true,
                ..Filter::default()
            },
        },
    )
    .await;
    let version = h.room().await.version;
    h.command(
        0,
        Command::Communicate {
            to: None,
            message: Communication {
                intent: Intent::Strategy,
                resources: None,
                text: Some("noisy".into()),
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
    let mine = h.service.view(&h.id, &identity(1).subject).await.unwrap();
    let other = h.service.view(&h.id, &identity(0).subject).await.unwrap();
    assert!(mine.offers.is_empty());
    assert!(!other.filters.unwrap()[1].shadow_mute);
    assert_eq!(other.offers.len(), 1);
    let page = h
        .service
        .history(&h.id, &identity(1).subject, version, 100)
        .await
        .unwrap();
    assert!(
        page.events
            .iter()
            .all(|b| !b.events.iter().any(|e| matches!(
                e,
                PublicEvent::Message { .. } | PublicEvent::TradeOffered { .. }
            )))
    );
    let id = other.offers[0].id.clone();
    let r = h
        .service
        .execute(
            &h.id,
            identity(1),
            request(
                h.room().await.version,
                Command::AcceptTrade { offer_id: id },
            ),
            2,
        )
        .await;
    assert_eq!(r.unwrap_err().0.code, ErrorCode::TradeUnavailable);
}
#[tokio::test]
async fn spectators_cannot_act_and_direct_messages_stay_private() {
    let h = Harness::trading().await;
    h.command(
        4,
        Command::Join {
            name: "Observer".into(),
            spectator: true,
        },
    )
    .await;
    let version = h.room().await.version;
    h.command(
        0,
        Command::Communicate {
            to: Some(vec![1]),
            message: Communication {
                intent: Intent::Chat,
                resources: None,
                text: Some("private".into()),
            },
        },
    )
    .await;
    for p in [2, 4] {
        let page = h
            .service
            .history(&h.id, &identity(p).subject, version, 100)
            .await
            .unwrap();
        assert!(page.events[0].events.is_empty());
    }
    let r = h
        .service
        .execute(
            &h.id,
            identity(4),
            request(
                h.room().await.version,
                Command::Act {
                    action: Action::EndTurn,
                },
            ),
            1,
        )
        .await;
    assert_eq!(r.unwrap_err().0.code, ErrorCode::Forbidden);
}
