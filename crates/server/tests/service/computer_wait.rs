use super::{choose, next_view};
use crate::{auth::Identity, model::Input, reducer};
use settlers_protocol::*;
use std::collections::HashMap;

fn fixture() -> crate::model::Room {
    let settings = Settings {
        turn_seconds: 0,
        ..Settings::default()
    };
    let (mut room, _) = reducer::create(
        "trade-wait".into(),
        CreateGame {
            name: "Trade wait".into(),
            settings,
            research: false,
            host_as_player: true,
            player_name: None,
            seed: None,
        },
        Identity {
            subject: "player-0".into(),
            name: "AI".into(),
        },
        [7; 32],
        1,
    )
    .unwrap();
    let host = room.members[0].clone();
    for seat in 1..4 {
        let mut member = host.clone();
        member.identity.subject = format!("player-{seat}");
        member.identity.name = format!("Human {seat}");
        member.seat = Some(seat);
        room.members.push(member);
    }
    room.members[0].computer_policy = Some(ComputerPolicy::Easy);
    let mut game = settlers_engine::State::new(settings, [7; 32]).unwrap();
    while game.turn == 0 {
        game = settlers_engine::timeout(&game).unwrap().0;
    }
    game.phase = Phase::Main;
    game.current = 0;
    for player in &mut game.players {
        player.resources = Resources([2; 5]);
    }
    game.players[0].resources = Resources([0, 0, 0, 0, 1]);
    game.bank = Resources([13, 13, 13, 13, 12]);
    room.game = Some(game);
    command(
        &room,
        0,
        Command::OfferTrade {
            to: vec![1, 2],
            give: Resources([0, 0, 0, 0, 1]),
            receive: Resources([1, 0, 0, 0, 0]),
            expires_in_ms: None,
            message: None,
        },
        1_000,
    )
}

fn command(room: &crate::model::Room, seat: u8, command: Command, now: u64) -> crate::model::Room {
    let request = CommandRequest {
        request_id: uuid::Uuid::from_u128(u128::from(room.version)).to_string(),
        expected_version: room.version,
        command,
    };
    let input = if room.members[seat as usize].policy().is_some() {
        Input::ComputerCommand { seat, request }
    } else {
        Input::Command {
            identity: room.members[seat as usize].identity.clone(),
            request,
        }
    };
    reducer::reduce(room, input, now).unwrap().0
}

#[test]
fn open_offers_hold_the_turn_through_thinking_and_partial_declines() {
    let room = fixture();
    let id = room.offers[0].public.id.clone();
    assert!(
        next_view(&room, &HashMap::new()).is_none(),
        "a human must have time to respond"
    );
    let room = command(
        &room,
        1,
        Command::ThinkTrade {
            offer_id: id.clone(),
        },
        10_000,
    );
    assert!(
        next_view(&room, &HashMap::new()).is_none(),
        "thinking is not a completed response"
    );
    let room = command(
        &room,
        2,
        Command::RejectTrade {
            offer_id: id.clone(),
        },
        20_000,
    );
    assert!(
        next_view(&room, &HashMap::new()).is_none(),
        "one human is still considering the offer"
    );
    let room = command(&room, 1, Command::RejectTrade { offer_id: id }, 21_000);
    assert_eq!(next_view(&room, &HashMap::new()).unwrap().0, 0);
}

#[test]
fn acceptance_or_the_existing_offer_deadline_resumes_the_computer() {
    let room = fixture();
    let id = room.offers[0].public.id.clone();
    let accepted = command(&room, 1, Command::AcceptTrade { offer_id: id }, 20_000);
    assert_eq!(next_view(&accepted, &HashMap::new()).unwrap().0, 0);
    let before_expiry = command(
        &room,
        1,
        Command::ThinkTrade {
            offer_id: room.offers[0].public.id.clone(),
        },
        30_999,
    );
    assert!(next_view(&before_expiry, &HashMap::new()).is_none());
    let expired = reducer::reduce(&room, Input::Timeout, 31_000).unwrap().0;
    assert_eq!(expired.offers[0].public.status, OfferStatus::Expired);
    assert_eq!(next_view(&expired, &HashMap::new()).unwrap().0, 0);
}

#[test]
fn computer_recipients_remain_schedulable_while_the_sender_waits() {
    let mut room = fixture();
    room.members[2].computer_policy = Some(ComputerPolicy::Easy);
    assert_eq!(next_view(&room, &HashMap::new()).unwrap().0, 2);
    let id = room.offers[0].public.id.clone();
    let room = command(&room, 2, Command::RejectTrade { offer_id: id }, 2_000);
    assert!(next_view(&room, &HashMap::new()).is_none());
}

#[test]
fn waiting_does_not_extend_the_authoritative_turn_deadline() {
    let mut room = fixture();
    room.deadline_ms = Some(12_000);
    assert!(next_view(&room, &HashMap::new()).is_none());
    let timed_out = reducer::reduce(&room, Input::Timeout, 12_000).unwrap().0;
    assert_ne!(timed_out.game.unwrap().current, 0);
    assert_eq!(timed_out.offers[0].public.status, OfferStatus::Expired);
}

#[test]
fn a_counter_offer_cannot_trigger_end_turn_while_the_original_is_open() {
    let room = fixture();
    let room = command(
        &room,
        1,
        Command::OfferTrade {
            to: vec![0],
            give: Resources([0, 1, 0, 0, 0]),
            receive: Resources([0, 0, 0, 0, 2]),
            expires_in_ms: None,
            message: None,
        },
        2_000,
    );
    let (seat, policy, view) = next_view(&room, &HashMap::new()).unwrap();
    assert_eq!(seat, 0);
    assert_eq!(
        choose(seat, policy, view, Some(room.game.unwrap().turn))
            .unwrap()
            .2,
        None
    );
}

#[test]
fn a_favorable_counter_offer_can_still_be_accepted_while_waiting() {
    let room = fixture();
    let room = command(
        &room,
        1,
        Command::OfferTrade {
            to: vec![0],
            give: Resources([2, 2, 2, 2, 0]),
            receive: Resources([0, 0, 0, 0, 1]),
            expires_in_ms: None,
            message: None,
        },
        2_000,
    );
    let id = room.offers.last().unwrap().public.id.clone();
    let (seat, policy, view) = next_view(&room, &HashMap::new()).unwrap();
    assert_eq!(
        choose(seat, policy, view, Some(room.game.unwrap().turn))
            .unwrap()
            .2,
        Some(Command::AcceptTrade { offer_id: id })
    );
}
