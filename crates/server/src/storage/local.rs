use super::{Archive, Store};
use crate::{
    error::{Error, Result},
    model::{Record, Room},
};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct LocalStore {
    connection: Arc<Mutex<Connection>>,
}

const SCHEMA: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = FULL;

    CREATE TABLE IF NOT EXISTS games (
        id TEXT PRIMARY KEY,
        version INTEGER NOT NULL,
        state TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS outbox (
        game_id TEXT NOT NULL,
        sequence INTEGER NOT NULL,
        event TEXT NOT NULL,
        PRIMARY KEY (game_id, sequence)
    );
";

impl LocalStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::storage)?;
        }
        let connection = Connection::open(path).map_err(Error::storage)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(Error::storage)?;
        connection.execute_batch(SCHEMA).map_err(Error::storage)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(Error::storage)?;
            f(&mut connection)
        })
        .await
        .map_err(Error::storage)?
    }
}
#[async_trait]
impl Store for LocalStore {
    async fn load(&self, id: &str) -> Result<Option<Room>> {
        let id = id.to_owned();
        self.run(move |c| {
            let json: Option<String> = c
                .query_row("SELECT state FROM games WHERE id=?1", [id], |r| r.get(0))
                .optional()
                .map_err(Error::storage)?;
            json.map(|json| {
                let room: Room = serde_json::from_str(&json).map_err(Error::storage)?;
                room.validate()?;
                Ok(room)
            })
            .transpose()
        })
        .await
    }
    async fn ids(&self) -> Result<Vec<String>> {
        self.run(|c| {
            let mut stmt = c
                .prepare("SELECT id FROM games ORDER BY id")
                .map_err(Error::storage)?;
            stmt.query_map([], |r| r.get(0))
                .map_err(Error::storage)?
                .collect::<std::result::Result<_, _>>()
                .map_err(Error::storage)
        })
        .await
    }
    async fn commit(&self, expected: u64, room: &Room, record: &Record) -> Result<()> {
        let id = room.id.clone();
        let version = i64::try_from(room.version).map_err(Error::storage)?;
        let state = serde_json::to_string(room).map_err(Error::storage)?;
        let event = serde_json::to_string(record).map_err(Error::storage)?;
        self.run(move |connection| {
            // Reserve the write lock before checking the version and outbox capacity.
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(Error::storage)?;
            let current: Option<i64> = transaction
                .query_row("SELECT version FROM games WHERE id=?1", [&id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(Error::storage)?;
            let current = current.unwrap_or(0) as u64;
            if current != expected {
                return Err(Error::conflict(current));
            }
            let pending: i64 = transaction
                .query_row(
                    "SELECT count(*) FROM outbox WHERE game_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(Error::storage)?;
            if pending >= super::OUTBOX_LIMIT as i64 {
                return Err(Error::new(
                    settlers_protocol::ErrorCode::Backpressure,
                    "archive is behind; game is paused until pending events are delivered",
                ));
            }

            // Both writes roll back if either statement or the final commit fails.
            transaction
                .execute(
                    "INSERT INTO games (id, version, state) VALUES (?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE
                     SET version=excluded.version, state=excluded.state",
                    params![id, version, state],
                )
                .map_err(Error::storage)?;
            transaction
                .execute(
                    "INSERT INTO outbox (game_id, sequence, event) VALUES (?1, ?2, ?3)",
                    params![id, version, event],
                )
                .map_err(Error::storage)?;
            transaction.commit().map_err(Error::storage)
        })
        .await
    }
    async fn pending(&self, id: &str) -> Result<Vec<Record>> {
        let id = id.to_owned();
        self.run(move |c| {
            let mut stmt = c
                .prepare("SELECT event FROM outbox WHERE game_id=?1 ORDER BY sequence")
                .map_err(Error::storage)?;
            let rows = stmt
                .query_map([id], |r| r.get::<_, String>(0))
                .map_err(Error::storage)?;
            rows.map(|r| serde_json::from_str(&r.map_err(Error::storage)?).map_err(Error::storage))
                .collect()
        })
        .await
    }
    async fn prune(&self, id: &str, through: u64) -> Result<()> {
        let id = id.to_owned();
        self.run(move |c| {
            c.execute(
                "DELETE FROM outbox WHERE game_id=?1 AND sequence<=?2",
                params![id, i64::try_from(through).map_err(Error::storage)?],
            )
            .map_err(Error::storage)?;
            Ok(())
        })
        .await
    }
}

pub struct LocalArchive {
    root: PathBuf,
}
impl LocalArchive {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}
fn valid_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id).map(|_| ()).map_err(|_| {
        Error::new(
            settlers_protocol::ErrorCode::InvalidRequest,
            "invalid game ID",
        )
    })
}
#[async_trait]
impl Archive for LocalArchive {
    async fn deliver(&self, records: &[Record]) -> Result<()> {
        let records = records.to_vec();
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            for record in records {
                valid_id(&record.game_id)?;
                let dir = root.join(format!("game_id={}", record.game_id));
                std::fs::create_dir_all(&dir).map_err(Error::storage)?;
                let path = dir.join(format!("{:020}.json", record.sequence));
                if path.exists() {
                    let old: Record =
                        serde_json::from_slice(&std::fs::read(&path).map_err(Error::storage)?)
                            .map_err(Error::storage)?;
                    if old != record {
                        return Err(Error::new(
                            settlers_protocol::ErrorCode::Internal,
                            "archive record conflict",
                        ));
                    }
                    continue;
                }
                let temporary = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(Error::storage)?;
                file.write_all(&super::encode_record(&record)?)
                    .map_err(Error::storage)?;
                file.write_all(b"\n").map_err(Error::storage)?;
                file.sync_all().map_err(Error::storage)?;
                std::fs::rename(temporary, path).map_err(Error::storage)?;
                std::fs::File::open(&dir)
                    .and_then(|f| f.sync_all())
                    .map_err(Error::storage)?;
            }
            Ok(())
        })
        .await
        .map_err(Error::storage)?
    }
    async fn read_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<Record>> {
        valid_id(id)?;
        let Some((first, last)) = super::sequence_window(after, limit) else {
            return Ok(vec![]);
        };
        let dir = self.root.join(format!("game_id={id}"));
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut records = Vec::new();
            for sequence in first..=last {
                let bytes = match std::fs::read(dir.join(format!("{sequence:020}.json"))) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(Error::storage(error)),
                };
                let record: Record = serde_json::from_slice(&bytes).map_err(Error::storage)?;
                if record.game_id != id || record.sequence != sequence {
                    return Err(Error::storage("archive key does not match record"));
                }
                records.push(record);
            }
            Ok(records)
        })
        .await
        .map_err(Error::storage)?
    }
    async fn read(&self, id: &str) -> Result<Vec<Record>> {
        valid_id(id)?;
        let dir = self.root.join(format!("game_id={id}"));
        tokio::task::spawn_blocking(move || {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(e) => return Err(Error::storage(e)),
            };
            let mut records = vec![];
            for entry in entries {
                let path = entry.map_err(Error::storage)?.path();
                if path.extension().is_some_and(|e| e == "json") {
                    records.push(
                        serde_json::from_slice(&std::fs::read(path).map_err(Error::storage)?)
                            .map_err(Error::storage)?,
                    );
                }
            }
            super::merge_records(records)
        })
        .await
        .map_err(Error::storage)?
    }
}
