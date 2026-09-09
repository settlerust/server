use server::{auth::Identity, reducer};
use settlers_protocol::*;

fn request(research: bool, deterministic: bool, seed: Option<u32>) -> CreateGame {
    CreateGame {
        research,
        name: "Deterministic".into(),
        settings: Settings {
            deterministic,
            ..Settings::default()
        },
        host_as_player: !research,
        player_name: None,
        seed,
    }
}
fn host() -> Identity {
    Identity {
        subject: "host".into(),
        name: "Host".into(),
    }
}

#[test]
fn a_seed_is_accepted_only_for_deterministic_research_tables_and_stays_private() {
    let random = [7u8; 32];
    let (room, record) =
        reducer::create("g".into(), request(true, true, Some(5)), host(), random, 0).unwrap();
    assert_ne!(room.seed, random);
    assert_eq!(&room.seed[..4], &5u32.to_le_bytes());
    assert!(room.settings.deterministic);
    let json = serde_json::to_string(&record.public).unwrap();
    assert!(!json.contains("seed"));
    for (research, deterministic) in [(false, true), (true, false), (false, false)] {
        assert!(
            reducer::create(
                "g".into(),
                request(research, deterministic, Some(5)),
                host(),
                random,
                0
            )
            .is_err()
        );
    }
    let (plain, _) =
        reducer::create("g".into(), request(true, true, None), host(), random, 0).unwrap();
    assert_eq!(plain.seed, random);
}
