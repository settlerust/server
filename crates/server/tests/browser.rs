use server::browser::BrowserRoom;
use settlers_policies::{Policy, fast_builder::FastBuilder};
use settlers_protocol::*;

fn identity(seat: u8) -> String {
    serde_json::json!({ "subject": format!("p{seat}"), "name": format!("Player {seat}") })
        .to_string()
}
fn view(room: &BrowserRoom, seat: u8) -> GameView {
    serde_json::from_str(&room.view(&format!("p{seat}")).unwrap()).unwrap()
}
fn envelope(room: &BrowserRoom, command: Command) -> String {
    serde_json::to_string(&CommandRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        expected_version: view(room, 0).version,
        command,
    })
    .unwrap()
}
fn command(room: &mut BrowserRoom, seat: u8, command: Command) {
    let request = envelope(room, command);
    room.command(&identity(seat), &request, 1000.0).unwrap();
}
fn started() -> BrowserRoom {
    let mut room = BrowserRoom::new();
    room.create("local-test".into(), &serde_json::json!({ "name": "Browser test", "host_as_player": true,
        "settings": { "players": 4, "points_to_win": 10, "discard_limit": 7, "turn_seconds": 120 }
    }).to_string(), &identity(0), &[11; 32], 1000.0).unwrap();
    for p in 1..4 {
        command(
            &mut room,
            p,
            Command::JoinAgent {
                name: format!("AI {p}"),
            },
        );
    }
    for p in 0..4 {
        room.presence(format!("p{p}"), true, 1000.0).unwrap();
        command(&mut room, p, Command::SetReady { ready: true });
    }
    command(&mut room, 0, Command::RollForOrder);
    command(&mut room, 0, Command::Start);
    room
}
#[test]
fn browser_commands_preserve_redaction_concurrency_and_durable_receipts() {
    let mut room = started();
    let request = envelope(
        &room,
        Command::SetFilter {
            player: 1,
            filter: Filter::default(),
        },
    );
    let receipt: Receipt =
        serde_json::from_str(&room.command(&identity(0), &request, 1000.0).unwrap()).unwrap();
    let stale: ApiError =
        serde_json::from_str(&room.command(&identity(1), &request, 1000.0).unwrap_err()).unwrap();
    assert_eq!(stale.code, ErrorCode::Conflict);
    let version = view(&room, 0).version;
    let deadline = view(&room, 0).deadline_ms.unwrap();
    room = BrowserRoom::restore(&room.checkpoint().unwrap(), 1000.0, 6000.0).unwrap();
    let duplicate: Receipt =
        serde_json::from_str(&room.command(&identity(0), &request, 6000.0).unwrap()).unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.version, receipt.version);
    assert_eq!(view(&room, 0).version, version);
    assert_eq!(view(&room, 0).deadline_ms.unwrap(), deadline + 5000);
    for p in 0..4 {
        let projection = view(&room, p);
        assert!(projection.research_hands.is_none());
        let game = projection.game.unwrap();
        assert_eq!(game.seat, Some(p));
        assert!(game.own.is_some());
        let json = room.view(&format!("p{p}")).unwrap();
        for secret in ["\"seed\"", "\"rng\"", "\"receipts\"", "\"state_hash\""] {
            assert!(!json.contains(secret));
        }
    }
    assert!(room.view("outsider").is_err());
    assert!(room.events("outsider", 0.0, 100).is_err());
    assert!(room.events("outsider", f64::MAX, 100).is_err());
}
#[test]
fn browser_adapter_finishes_a_ten_point_game_and_paginates_history() {
    let mut room = started();
    let mut policies = [FastBuilder, FastBuilder, FastBuilder, FastBuilder];
    for _ in 0..10000 {
        if matches!(
            view(&room, 0).game.unwrap().phase,
            settlers_engine::Phase::Finished { .. }
        ) {
            break;
        }
        let mut moved = false;
        for p in 0..4 {
            let projection = view(&room, p);
            if let Some(action) = policies[p as usize].choose(&projection) {
                command(&mut room, p, action);
                moved = true;
                break;
            }
        }
        assert!(moved, "all policies stalled");
    }
    let finished = view(&room, 0);
    assert!(matches!(
        finished.game.unwrap().phase,
        settlers_engine::Phase::Finished { .. }
    ));
    let mut cursor = 0;
    while cursor < finished.version {
        let page: EventPage =
            serde_json::from_str(&room.events("p0", cursor as f64, 7).unwrap()).unwrap();
        assert!(!page.archive_pending);
        assert_eq!(page.events.first().unwrap().sequence, cursor + 1);
        assert!(page.events.len() <= 7);
        assert_eq!(page.current_version, finished.version);
        cursor = page.next_after;
    }
}
#[test]
fn browser_timeout_uses_the_official_reducer() {
    let mut room = started();
    let deadline = view(&room, 0).deadline_ms.unwrap();
    let before = view(&room, 0).version;
    assert!(!room.tick((deadline - 1) as f64).unwrap());
    assert!(room.tick(deadline as f64).unwrap());
    assert_eq!(view(&room, 0).version, before + 1);
}
