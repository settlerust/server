mod common;
use common::*;
use server::service::GameService;
use settlers_protocol::*;

fn chat(to: Option<Vec<u8>>) -> Command {
    Command::Communicate {
        to,
        message: Communication {
            intent: Intent::Chat,
            resources: None,
            text: Some("Hello".into()),
        },
    }
}

#[tokio::test]
async fn public_direct_and_trade_messages_share_a_durable_limit() {
    let h = Harness::trading().await;
    for index in 0..15 {
        h.command(0, chat(if index % 2 == 0 { None } else { Some(vec![1]) }))
            .await;
    }
    let offer = Command::OfferTrade {
        to: vec![1],
        give: Resources([1, 0, 0, 0, 0]),
        receive: Resources([0, 1, 0, 0, 0]),
        expires_in_ms: None,
        message: Some(Communication {
            intent: Intent::Chat,
            resources: None,
            text: Some("One last offer".into()),
        }),
    };
    let last = request(h.room().await.version, offer.clone());
    h.service
        .execute(&h.id, identity(0), last.clone(), 1)
        .await
        .unwrap();
    let version = h.room().await.version;
    let service = GameService::restore(h.store.clone(), h.archive.clone(), 2)
        .await
        .unwrap();
    assert!(
        service
            .execute(&h.id, identity(0), last, 2)
            .await
            .unwrap()
            .duplicate
    );
    for command in [chat(None), chat(Some(vec![1])), offer] {
        let rejected = service
            .execute(&h.id, identity(0), request(version, command), 2)
            .await
            .unwrap_err();
        assert_eq!(rejected.0.code, ErrorCode::Backpressure);
        assert_eq!(service.room(&h.id).await.unwrap().version, version);
    }
    // One player's allowance cannot silence another or block ordinary game actions.
    service
        .execute(&h.id, identity(1), request(version, chat(None)), 2)
        .await
        .unwrap();
    let room = service.room(&h.id).await.unwrap();
    service
        .execute(
            &h.id,
            identity(0),
            request(
                room.version,
                Command::CancelTrade {
                    offer_id: room.offers[0].public.id.clone(),
                },
            ),
            3,
        )
        .await
        .unwrap();
    let version = service.room(&h.id).await.unwrap().version;
    service
        .execute(&h.id, identity(0), request(version, chat(None)), 60_001)
        .await
        .unwrap();
}
