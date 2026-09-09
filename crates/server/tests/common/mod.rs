#![allow(dead_code)]
use server::{
    auth::Identity,
    model::Room,
    reducer,
    service::GameService,
    storage::{
        Store,
        local::{LocalArchive, LocalStore},
    },
};
use settlers_protocol::*;
use std::sync::Arc;
pub fn identity(p: u8) -> Identity {
    Identity {
        subject: format!("player-{p}"),
        name: format!("Player {p}"),
    }
}
pub fn request(version: u64, command: Command) -> CommandRequest {
    CommandRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        expected_version: version,
        command,
    }
}
pub struct Harness {
    pub dir: tempfile::TempDir,
    pub store: Arc<LocalStore>,
    pub archive: Arc<LocalArchive>,
    pub service: GameService,
    pub id: String,
}
impl Harness {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(LocalStore::open(&dir.path().join("state.sqlite3")).unwrap());
        let archive = Arc::new(LocalArchive::new(dir.path().join("archive")));
        let service = GameService::restore(store.clone(), archive.clone(), 0)
            .await
            .unwrap();
        let view = service
            .create(
                identity(0),
                CreateGame {
                    research: false,
                    name: "Test".into(),
                    settings: Settings {
                        turn_seconds: 86400,
                        ..Settings::default()
                    },
                    host_as_player: true,
                    player_name: None,
                },
                0,
            )
            .await
            .unwrap();
        Self {
            dir,
            store,
            archive,
            service,
            id: view.id,
        }
    }
    pub async fn command(&self, p: u8, command: Command) -> Receipt {
        let version = self.service.room(&self.id).await.unwrap().version;
        self.service
            .execute(&self.id, identity(p), request(version, command), 1)
            .await
            .unwrap()
    }
    pub async fn ready_all(&self) {
        for member in self
            .room()
            .await
            .members
            .iter()
            .filter(|m| m.seat.is_some())
        {
            let p = member.seat.unwrap();
            self.service
                .presence(
                    &self.id,
                    member.identity.subject.clone(),
                    format!("test-{p}"),
                    true,
                    0,
                )
                .await
                .unwrap();
            self.command(p, Command::SetReady { ready: true }).await;
        }
    }
    pub async fn start(&self) {
        for p in 1..4 {
            self.command(
                p,
                Command::Join {
                    name: format!("Player {p}"),
                    spectator: false,
                },
            )
            .await;
        }
        self.ready_all().await;
        self.command(0, Command::Start).await;
    }
    pub async fn trading() -> Self {
        let h = Self::new().await;
        h.start().await;
        let room = h.service.room(&h.id).await.unwrap();
        let mut fixture = room.clone();
        let game = fixture.game.as_mut().unwrap();
        while game.turn == 0 {
            *game = settlers_engine::timeout(game).unwrap().0;
        }
        game.phase = settlers_engine::Phase::Main;
        game.current = 0;
        for p in 0..4 {
            game.players[p].resources = Resources([2, 2, 2, 2, 2]);
        }
        game.bank = Resources([11; 5]);
        // Controlled authoritative fixture for concurrency tests; replay tests use real commands only.
        fixture.version += 1;
        let record = server::model::Record {
            schema: 1,
            engine_version: "base-v1".into(),
            game_id: fixture.id.clone(),
            sequence: fixture.version,
            timestamp_ms: 1,
            input: server::model::Input::Timeout,
            public: vec![],
            state_hash: reducer::state_hash(&fixture).unwrap(),
        };
        h.store
            .commit(room.version, &fixture, &record)
            .await
            .unwrap();
        let service = GameService::restore(h.store.clone(), h.archive.clone(), 1)
            .await
            .unwrap();
        Self { service, ..h }
    }
    pub async fn record_count(&self) -> usize {
        self.store.pending(&self.id).await.unwrap().len()
    }
    pub async fn room(&self) -> Room {
        self.service.room(&self.id).await.unwrap()
    }
}
