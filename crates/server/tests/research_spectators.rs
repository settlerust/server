mod common;
use common::*;
use server::storage::Store;
use server::{
    auth::Auth,
    config::now_ms,
    model::Input,
    reducer,
    transport::{App, router},
};
use settlers_client::{Client, ServerFrame};
use settlers_protocol::*;

#[tokio::test]
async fn research_spectators_get_live_hands_while_agents_remain_redacted_and_replay_preserves_mode()
{
    let h = Harness::new().await;
    let created = h
        .service
        .create(
            identity(9),
            CreateGame {
                name: "Research visibility".into(),
                research: true,
                settings: Settings::default(),
                host_as_player: false,
                player_name: None,
            },
            0,
        )
        .await
        .unwrap();
    let id = created.id;
    for p in 0..4 {
        let version = h.service.room(&id).await.unwrap().version;
        h.service
            .execute(
                &id,
                identity(p),
                request(
                    version,
                    Command::JoinAgent {
                        name: format!("Agent {p}"),
                    },
                ),
                1,
            )
            .await
            .unwrap();
    }
    let room = h.service.room(&id).await.unwrap();
    assert!(
        reducer::reduce(
            &room,
            Input::Command {
                identity: identity(8),
                request: request(
                    room.version,
                    Command::Join {
                        name: "Human".into(),
                        spectator: false
                    }
                )
            },
            2
        )
        .is_err()
    );
    h.service
        .execute(&id, identity(9), request(room.version, Command::Start), 2)
        .await
        .unwrap();
    let auth = Auth::new(vec![42; 32], true).unwrap();
    let observer = auth.issue(&identity(9), now_ms() / 1000).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = App {
        auth,
        games: h.service.clone(),
    };
    let task = tokio::spawn(async move {
        axum::serve(listener, router(app)).await.unwrap();
    });
    let client = Client::new(&base, observer.token).unwrap();
    let mut socket = client.connect(&id).await.unwrap();
    let ServerFrame::Snapshot { game, .. } = socket.next().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(game.research_hands.as_ref().unwrap().len(), 4);
    assert!(game.game.as_ref().unwrap().own.is_none());
    let version = h.service.room(&id).await.unwrap().version;
    // A command performs legal setup and makes the spectator receive another live snapshot.
    let room = h.service.room(&id).await.unwrap();
    let p = room.game.unwrap().current;
    let action = settlers_engine::legal_actions(&room.game.unwrap(), p)[0];
    h.service
        .execute(
            &id,
            identity(p),
            request(version, Command::Act { action }),
            3,
        )
        .await
        .unwrap();
    loop {
        if let ServerFrame::Snapshot { game, .. } = socket.next().await.unwrap().unwrap()
            && game.version > version
        {
            let source = h.service.room(&id).await.unwrap().game.unwrap();
            for shown in game.research_hands.unwrap() {
                assert_eq!(shown.hand, source.players[shown.seat as usize]);
            }
            break;
        }
    }
    let agent = h.service.view(&id, &identity(p).subject).await.unwrap();
    assert!(agent.research_hands.is_none());
    assert!(agent.game.as_ref().unwrap().own.is_some());
    let json = serde_json::to_string(&agent).unwrap();
    assert!(!json.contains("research_hands"));
    assert!(!json.contains("random"));
    socket.close().await.unwrap();
    task.abort();
    let records = h.store.pending(&id).await.unwrap();
    let replayed = reducer::replay(&records).unwrap();
    assert!(replayed.research);
    assert!(
        reducer::view(&replayed, &identity(9).subject)
            .unwrap()
            .research_hands
            .is_some()
    );
    let standard = h.service.view(&h.id, &identity(0).subject).await.unwrap();
    assert!(!standard.research);
    assert!(standard.research_hands.is_none());
}
