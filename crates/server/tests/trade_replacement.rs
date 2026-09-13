mod common;
use common::*;
use settlers_protocol::*;
fn offer(give: u8, receive: u8) -> Command {
    Command::OfferTrade {
        to: vec![1, 2],
        give: Resources([give, 0, 0, 0, 0]),
        receive: Resources([0, receive, 0, 0, 0]),
        expires_in_ms: None,
        message: None,
    }
}
#[tokio::test]
async fn better_offers_replace_worse_ones_and_acceptance_invalidates_unfunded_promises() {
    let h = Harness::trading().await;
    h.command(0, offer(1, 2)).await;
    let old = h.room().await.offers.last().unwrap().public.id.clone();
    h.command(0, offer(2, 1)).await;
    let room = h.room().await;
    let new = room.offers.last().unwrap().public.id.clone();
    assert_eq!(
        room.offers
            .iter()
            .find(|o| o.public.id == old)
            .unwrap()
            .public
            .status,
        OfferStatus::Cancelled
    );
    let result = h
        .service
        .execute(
            &h.id,
            identity(1),
            request(room.version - 1, Command::AcceptTrade { offer_id: old }),
            2,
        )
        .await;
    assert!(result.is_err());
    // A different promise consumes the same resources and must be retracted on acceptance.
    h.command(
        0,
        Command::OfferTrade {
            to: vec![1],
            give: Resources([2, 0, 0, 0, 0]),
            receive: Resources([0, 0, 1, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
    )
    .await;
    h.command(1, Command::AcceptTrade { offer_id: new }).await;
    assert!(
        h.service
            .view(&h.id, &identity(0).subject)
            .await
            .unwrap()
            .offers
            .is_empty()
    );
    let history = h
        .service
        .history(&h.id, &identity(0).subject, 0, 100)
        .await
        .unwrap();
    assert!(
        history
            .events
            .iter()
            .any(|b| b.events.iter().any(|e| matches!(
                e,
                PublicEvent::TradeClosed {
                    reason: TradeCloseReason::ResourcesChanged,
                    ..
                }
            )))
    );
}
#[tokio::test]
async fn acceptance_before_replacement_wins_without_a_second_trade() {
    let h = Harness::trading().await;
    h.command(0, offer(1, 2)).await;
    let old = h.room().await.offers.last().unwrap().public.id.clone();
    let version = h.room().await.version;
    h.command(1, Command::AcceptTrade { offer_id: old }).await;
    let before = h.room().await;
    assert!(
        h.service
            .execute(&h.id, identity(0), request(version, offer(1, 1)), 3)
            .await
            .is_err()
    );
    let result = h
        .service
        .execute(&h.id, identity(0), request(before.version, offer(1, 1)), 3)
        .await
        .unwrap_err();
    assert_eq!(result.0.code, ErrorCode::TradeTaken);
    assert_eq!(result.0.winner, Some(1));
    assert_eq!(before.version, h.room().await.version);
}
