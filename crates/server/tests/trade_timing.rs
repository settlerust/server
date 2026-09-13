mod common;
use common::*;
use server::{model::Input, reducer, service::GameService};
use settlers_protocol::*;

fn offer(to: Vec<u8>, lifetime: u32) -> Command {
    Command::OfferTrade {
        to,
        give: Resources([1, 0, 0, 0, 0]),
        receive: Resources([0, 19, 0, 0, 0]),
        expires_in_ms: Some(lifetime),
        message: None,
    }
}

#[tokio::test]
async fn unfunded_recipients_can_think_pass_and_counter_without_automatic_rejection() {
    let h = Harness::trading().await;
    h.command(0, offer(vec![1, 2], 15_000)).await;
    let offered = h.room().await;
    let id = offered.offers.last().unwrap().public.id.clone();
    // Every recipient has just two lumber. Publishing nineteen asks no questions about their hands.
    for p in 0..3 {
        let view = h.service.view(&h.id, &identity(p).subject).await.unwrap();
        assert_eq!(view.offers.len(), 1);
        assert!(view.offers[0].declined.is_empty());
        assert!(view.offers[0].thinking.is_empty());
    }
    h.command(
        1,
        Command::ThinkTrade {
            offer_id: id.clone(),
        },
    )
    .await;
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), 2)
        .await
        .unwrap();
    let view = restored.view(&h.id, &identity(0).subject).await.unwrap();
    assert_eq!(view.offers[0].thinking, vec![1]);
    let before = h.room().await;
    let failed = h
        .service
        .execute(
            &h.id,
            identity(1),
            request(
                before.version,
                Command::AcceptTrade {
                    offer_id: id.clone(),
                },
            ),
            2,
        )
        .await;
    assert!(failed.is_err());
    assert_eq!(h.room().await.version, before.version);
    assert!(h.room().await.offers[0].public.declined.is_empty());
    // Off-turn counters use the existing command, with arbitrary requested quantities as well.
    h.command(
        1,
        Command::OfferTrade {
            to: vec![0],
            give: Resources([0, 1, 0, 0, 0]),
            receive: Resources([19, 0, 0, 0, 0]),
            expires_in_ms: Some(60_000),
            message: None,
        },
    )
    .await;
    assert_eq!(h.room().await.offers.last().unwrap().public.from, 1);
    h.command(
        1,
        Command::RejectTrade {
            offer_id: id.clone(),
        },
    )
    .await;
    let passed = h.room().await;
    assert_eq!(passed.offers[0].public.declined, vec![1]);
    assert!(passed.offers[0].public.thinking.is_empty());
    let denied = h
        .service
        .execute(
            &h.id,
            identity(1),
            request(
                passed.version,
                Command::ThinkTrade {
                    offer_id: id.clone(),
                },
            ),
            2,
        )
        .await
        .unwrap_err();
    assert_eq!(denied.0.code, ErrorCode::TradeUnavailable);
    h.command(2, Command::RejectTrade { offer_id: id }).await;
    assert_eq!(
        h.room().await.offers[0].public.status,
        OfferStatus::Cancelled
    );
}

#[tokio::test]
async fn offer_deadlines_expire_independently_and_reject_late_acceptance_after_restore() {
    let h = Harness::trading().await;
    h.command(0, offer(vec![1], 15_000)).await;
    h.command(0, offer(vec![2], 60_000)).await;
    let before = h.room().await;
    assert_eq!(before.next_deadline_ms(), Some(15_001));
    let first = &before.offers[0].public;
    assert_eq!(first.created_at_ms, Some(1));
    assert_eq!(first.expires_at_ms, Some(15_001));
    assert!(reducer::reduce(&before, Input::Timeout, 15_000).is_err());
    let (expired, record) = reducer::reduce(&before, Input::Timeout, 15_001).unwrap();
    assert_eq!(expired.offers[0].public.status, OfferStatus::Expired);
    assert_eq!(expired.offers[1].public.status, OfferStatus::Open);
    assert_eq!(expired.next_deadline_ms(), Some(60_001));
    assert_eq!(before.game, expired.game);
    assert_eq!(before.deadline_ms, expired.deadline_ms);
    assert_eq!(
        record.public,
        vec![PublicEvent::TradeClosed {
            offer_id: first.id.clone(),
            reason: TradeCloseReason::Expired,
            winner: None
        }]
    );
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), 15_001)
        .await
        .unwrap();
    let result = restored
        .execute(
            &h.id,
            identity(1),
            request(
                before.version,
                Command::AcceptTrade {
                    offer_id: first.id.clone(),
                },
            ),
            15_001,
        )
        .await
        .unwrap_err();
    assert_eq!(result.0.code, ErrorCode::TradeUnavailable);
    let sender = restored.view(&h.id, &identity(0).subject).await.unwrap();
    assert_eq!(sender.offers.len(), 1);
    assert_eq!(sender.offers[0].to, vec![2]);
}

#[tokio::test]
async fn lifetimes_are_bounded_and_clamped_to_the_turn_and_thinking_is_addressed_only() {
    let h = Harness::trading().await;
    for invalid in [0, 999, 300_001] {
        let room = h.room().await;
        assert_eq!(
            h.service
                .execute(
                    &h.id,
                    identity(0),
                    request(room.version, offer(vec![1], invalid)),
                    1
                )
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::InvalidRequest
        );
    }
    let mut before = h.room().await;
    before.deadline_ms = Some(10_000);
    let (room, _) = reducer::reduce(
        &before,
        Input::Command {
            identity: identity(0),
            request: request(before.version, offer(vec![1], 60_000)),
        },
        1,
    )
    .unwrap();
    assert_eq!(room.offers[0].public.expires_at_ms, Some(10_000));
    let (ended, _) = reducer::reduce(&room, Input::Timeout, 10_000).unwrap();
    assert_eq!(ended.offers[0].public.status, OfferStatus::Expired);
    assert_ne!(ended.game.unwrap().turn, room.game.unwrap().turn);
    h.command(0, offer(vec![1], 15_000)).await;
    let room = h.room().await;
    for p in [0, 2] {
        assert_eq!(
            h.service
                .execute(
                    &h.id,
                    identity(p),
                    request(
                        room.version,
                        Command::ThinkTrade {
                            offer_id: room.offers[0].public.id.clone()
                        }
                    ),
                    2
                )
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::TradeUnavailable
        );
    }
}

#[tokio::test]
async fn local_checkpoint_preserves_remaining_offer_time_and_consideration() {
    let h = Harness::trading().await;
    h.command(0, offer(vec![1], 15_000)).await;
    let id = h.room().await.offers[0].public.id.clone();
    h.command(1, Command::ThinkTrade { offer_id: id }).await;
    let checkpoint = serde_json::json!({"room": h.room().await, "records": []});
    let mut local =
        server::browser::BrowserRoom::restore(&checkpoint.to_string(), 5_001.0, 105_001.0).unwrap();
    let view: GameView = serde_json::from_str(&local.view(&identity(0).subject).unwrap()).unwrap();
    assert_eq!(view.offers[0].created_at_ms, Some(100_001));
    assert_eq!(view.offers[0].expires_at_ms, Some(115_001));
    assert_eq!(view.offers[0].thinking, vec![1]);
    assert!(!local.tick(115_000.0).unwrap());
    assert!(local.tick(115_001.0).unwrap());
    let after: GameView = serde_json::from_str(&local.view(&identity(0).subject).unwrap()).unwrap();
    assert!(after.offers.is_empty());
    assert_eq!(after.game.unwrap().turn, view.game.unwrap().turn);
}
