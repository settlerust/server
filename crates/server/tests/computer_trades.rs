mod common;
use common::*;
use server::{
    config::now_ms,
    model::{Input, Record},
    reducer,
    service::GameService,
    storage::Store,
};
use settlers_protocol::*;
use std::time::Duration;

async fn trading(policy: ComputerPolicy) -> Harness {
    trading_with_counter(policy, false).await
}

async fn trading_with_counter(policy: ComputerPolicy, counter: bool) -> Harness {
    let h = Harness::trading().await;
    let before = h.room().await;
    let mut room = before.clone();
    room.settings.turn_seconds = 0;
    room.game.as_mut().unwrap().settings.turn_seconds = 0;
    room.deadline_ms = None;
    for member in &mut room.members {
        member.disconnected_at_ms = None;
        member.resolution = None;
        member.connected = false;
        if member.seat.is_some_and(|seat| seat > 0) {
            member.computer_policy = Some(policy);
        }
    }
    if counter {
        room.offers.push(server::model::Offer {
            public: TradeOffer {
                id: "computer-counter".into(),
                from: 1,
                to: vec![0],
                give: Resources([0, 0, 0, 0, 1]),
                receive: Resources([2, 0, 0, 0, 0]),
                message: None,
                status: OfferStatus::Open,
                thinking: vec![],
                declined: vec![],
                created_at_ms: Some(now_ms()),
                expires_at_ms: Some(now_ms() + 300_000),
            },
            rejected: 0,
            turn: Some(room.game.unwrap().turn),
        });
    }
    // Controlled hands isolate the hosted actor from random rolls and setup choices.
    room.version += 1;
    let record = Record {
        schema: 1,
        engine_version: server::model::ReplayVersion::CURRENT,
        game_id: room.id.clone(),
        sequence: room.version,
        timestamp_ms: now_ms(),
        input: Input::Timeout,
        public: vec![],
        state_hash: reducer::state_hash(&room).unwrap(),
    };
    h.store
        .commit(before.version, &room, &record)
        .await
        .unwrap();
    let service = GameService::restore(h.store.clone(), h.archive.clone(), now_ms())
        .await
        .unwrap();
    service
        .presence(&h.id, identity(0).subject, "human".into(), true, now_ms())
        .await
        .unwrap();
    Harness { service, ..h }
}

async fn offer(h: &Harness, to: Vec<u8>, give: Resources, receive: Resources) -> String {
    h.service
        .execute(
            &h.id,
            identity(0),
            request(
                h.room().await.version,
                Command::OfferTrade {
                    to,
                    give,
                    receive,
                    expires_in_ms: Some(300_000),
                    message: None,
                },
            ),
            now_ms(),
        )
        .await
        .unwrap();
    h.room().await.offers.last().unwrap().public.id.clone()
}

async fn answered(h: &Harness, id: &str) -> TradeOffer {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            h.service.tick(now_ms()).await.unwrap();
            let room = h.room().await;
            let offer = &room
                .offers
                .iter()
                .find(|o| o.public.id == id)
                .unwrap()
                .public;
            if offer.status != OfferStatus::Open {
                return offer.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("hosted computers must answer an offer during the human's turn")
}

#[tokio::test]
async fn hosted_expert_accepts_a_favorable_offer_outside_its_turn() {
    let h = trading(ComputerPolicy::Expert).await;
    let before = h.room().await.game.unwrap();
    assert!(settlers_engine::legal_actions(&before, 1).is_empty());
    let give = Resources([2, 2, 2, 2, 0]);
    let receive = Resources([0, 0, 0, 0, 1]);
    let id = offer(&h, vec![1], give, receive).await;
    assert_eq!(
        answered(&h, &id).await.status,
        OfferStatus::Accepted { player: 1 }
    );
    let room = h.room().await;
    let game = room.game.unwrap();
    assert_eq!(game.current, 0);
    assert_eq!(game.players[0].resources, Resources([0, 0, 0, 0, 3]));
    assert_eq!(game.players[1].resources, Resources([4, 4, 4, 4, 1]));
    let records = h.store.pending(&h.id).await.unwrap();
    assert!(records.iter().any(|record| matches!(&record.input,
        Input::ComputerCommand { seat: 1, request }
        if matches!(&request.command, Command::AcceptTrade { offer_id } if offer_id == &id)
    )));
    let restored = GameService::restore(h.store.clone(), h.archive.clone(), now_ms())
        .await
        .unwrap();
    assert_eq!(
        restored
            .room(&h.id)
            .await
            .unwrap()
            .offers
            .last()
            .unwrap()
            .public
            .status,
        OfferStatus::Accepted { player: 1 }
    );
}

#[tokio::test]
async fn every_hosted_policy_explicitly_declines_unpayable_offers_for_every_recipient() {
    for policy in [
        ComputerPolicy::Easy,
        ComputerPolicy::Medium,
        ComputerPolicy::Hard,
        ComputerPolicy::Expert,
    ] {
        let h = trading(policy).await;
        let id = offer(
            &h,
            vec![1, 2, 3],
            Resources([1, 0, 0, 0, 0]),
            Resources([0, 0, 0, 0, 10]),
        )
        .await;
        let response = answered(&h, &id).await;
        assert_eq!(response.status, OfferStatus::Cancelled);
        assert_eq!(response.declined, vec![1, 2, 3]);
        let records = h.store.pending(&h.id).await.unwrap();
        for seat in 1..4 {
            assert!(records.iter().any(|record| matches!(&record.input,
                Input::ComputerCommand { seat: p, request }
                if *p == seat && matches!(&request.command, Command::RejectTrade { offer_id } if offer_id == &id)
            )));
        }
        let history = h
            .service
            .history(&h.id, &identity(0).subject, 0, 100)
            .await
            .unwrap();
        assert!(history.events.iter().flat_map(|batch| &batch.events).any(|event| matches!(event,
            PublicEvent::TradeClosed { offer_id, reason: TradeCloseReason::Declined, .. } if offer_id == &id
        )));
    }
}

#[tokio::test]
async fn a_waiting_computer_does_not_block_other_recipients_and_reconsiders_new_offers() {
    let h = trading_with_counter(ComputerPolicy::Expert, true).await;
    let id = offer(
        &h,
        vec![1, 2],
        Resources([1, 0, 0, 0, 0]),
        Resources([0, 0, 0, 0, 10]),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            h.service.tick(now_ms()).await.unwrap();
            let room = h.room().await;
            let response = &room
                .offers
                .iter()
                .find(|o| o.public.id == id)
                .unwrap()
                .public;
            if response.declined.contains(&2) {
                assert_eq!(response.status, OfferStatus::Open);
                assert_eq!(response.declined, vec![2]);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("seat 2 should respond while seat 1 waits for its counteroffer");
    let better = offer(
        &h,
        vec![1],
        Resources([2, 2, 2, 2, 0]),
        Resources([0, 0, 0, 0, 1]),
    )
    .await;
    assert_eq!(
        answered(&h, &better).await.status,
        OfferStatus::Accepted { player: 1 }
    );
}

#[tokio::test]
async fn computer_recipients_answer_before_the_offering_computer_continues_its_turn() {
    let h = trading(ComputerPolicy::Expert).await;
    let mut room = h.room().await;
    room.members
        .iter_mut()
        .find(|member| member.seat == Some(0))
        .unwrap()
        .computer_policy = Some(ComputerPolicy::Expert);
    let before = room.version;
    let (room, record) = reducer::reduce(
        &room,
        Input::ComputerCommand {
            seat: 0,
            request: request(
                before,
                Command::OfferTrade {
                    to: vec![1],
                    give: Resources([2, 2, 2, 2, 0]),
                    receive: Resources([0, 0, 0, 0, 1]),
                    expires_in_ms: Some(300_000),
                    message: None,
                },
            ),
        },
        now_ms(),
    )
    .unwrap();
    h.store.commit(before, &room, &record).await.unwrap();
    let id = room.offers.last().unwrap().public.id.clone();
    let service = GameService::restore(h.store.clone(), h.archive.clone(), now_ms())
        .await
        .unwrap();
    let h = Harness { service, ..h };
    assert_eq!(
        answered(&h, &id).await.status,
        OfferStatus::Accepted { player: 1 }
    );
    let records = h.store.pending(&h.id).await.unwrap();
    let first = records
        .iter()
        .find(|record| {
            record.sequence > room.version && matches!(record.input, Input::ComputerCommand { .. })
        })
        .unwrap();
    assert!(
        matches!(&first.input, Input::ComputerCommand { seat: 1, request }
        if matches!(&request.command, Command::AcceptTrade { offer_id } if offer_id == &id))
    );
}

#[tokio::test]
async fn an_outgoing_offer_survives_restart_and_waits_for_a_human_response() {
    let h = Harness::trading().await;
    let before = h.room().await;
    let mut room = before.clone();
    room.settings.turn_seconds = 0;
    room.deadline_ms = None;
    let game = room.game.as_mut().unwrap();
    game.settings.turn_seconds = 0;
    game.players[0].resources = Resources([0, 0, 0, 0, 1]);
    game.bank = Resources([13, 13, 13, 13, 12]);
    for member in &mut room.members {
        member.connected = false;
        member.disconnected_at_ms = None;
        if member.seat == Some(0) {
            member.computer_policy = Some(ComputerPolicy::Easy);
        }
    }
    let (room, record) = reducer::reduce(
        &room,
        Input::ComputerCommand {
            seat: 0,
            request: request(
                room.version,
                Command::OfferTrade {
                    to: vec![1],
                    give: Resources([0, 0, 0, 0, 1]),
                    receive: Resources([1, 0, 0, 0, 0]),
                    expires_in_ms: None,
                    message: None,
                },
            ),
        },
        now_ms(),
    )
    .unwrap();
    h.store
        .commit(before.version, &room, &record)
        .await
        .unwrap();
    let offer_id = room.offers.last().unwrap().public.id.clone();
    let expected_game = room.game.unwrap();
    let service = GameService::restore(h.store.clone(), h.archive.clone(), now_ms())
        .await
        .unwrap();
    for seat in 1..4 {
        service
            .presence(
                &h.id,
                identity(seat).subject,
                format!("human-{seat}"),
                true,
                now_ms(),
            )
            .await
            .unwrap();
    }
    let h = Harness { service, ..h };
    h.service
        .execute(
            &h.id,
            identity(1),
            request(
                h.room().await.version,
                Command::ThinkTrade {
                    offer_id: offer_id.clone(),
                },
            ),
            now_ms(),
        )
        .await
        .unwrap();
    for _ in 0..10 {
        h.service.tick(now_ms()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let room = h.room().await;
        assert_eq!(
            room.game.unwrap(),
            expected_game,
            "waiting must preserve the turn and offered cards"
        );
        assert_eq!(room.offers.last().unwrap().public.status, OfferStatus::Open);
    }
    h.service
        .execute(
            &h.id,
            identity(1),
            request(h.room().await.version, Command::AcceptTrade { offer_id }),
            now_ms(),
        )
        .await
        .unwrap();
    let accepted = h.room().await;
    assert_eq!(
        accepted.offers.last().unwrap().public.status,
        OfferStatus::Accepted { player: 1 }
    );
    assert_eq!(
        accepted.game.unwrap().players[0].resources,
        Resources([1, 0, 0, 0, 0])
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            h.service.tick(now_ms()).await.unwrap();
            if h.store.pending(&h.id).await.unwrap().iter().any(|record| {
                record.sequence > accepted.version
                    && matches!(record.input, Input::ComputerCommand { seat: 0, .. })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("acceptance should let the computer continue immediately");
}
