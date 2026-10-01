use crate::{
    auth::Auth,
    error::{Error, Result},
};
use settlers_protocol::ErrorCode;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
/// Process configuration; local mode uses durable credentials in `data_dir`.
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub aws: bool,
    pub local: bool,
}

#[derive(Default)]
struct ConfigInput {
    mode: Option<String>,
    bind: Option<String>,
    storage: Option<String>,
    data_dir: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::parse(ConfigInput {
            mode: std::env::var("SETTLERS_MODE").ok(),
            bind: std::env::var("SETTLERS_BIND").ok(),
            storage: std::env::var("SETTLERS_STORAGE").ok(),
            data_dir: std::env::var_os("SETTLERS_DATA_DIR").map(PathBuf::from),
        })
    }

    // Parsing is separate from environment access so validation needs no global mutation.
    fn parse(input: ConfigInput) -> Result<Self> {
        let local = match input.mode.as_deref() {
            Some("production") => false,
            Some("local") | None => true,
            _ => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "SETTLERS_MODE must be local or production",
                ));
            }
        };
        let bind = input
            .bind
            .as_deref()
            .unwrap_or("127.0.0.1:8080")
            .parse()
            .map_err(|_| Error::new(ErrorCode::InvalidRequest, "invalid SETTLERS_BIND"))?;
        let storage = input.storage.as_deref().unwrap_or("local");
        if storage != "aws" && storage != "local" {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "SETTLERS_STORAGE must be local or aws",
            ));
        }
        Ok(Self {
            bind,
            data_dir: input.data_dir.unwrap_or_else(|| PathBuf::from("data")),
            aws: storage == "aws",
            local,
        })
    }
    pub fn auth(&self) -> Result<Auth> {
        if let Ok(key) = std::env::var("SETTLERS_JWT_PUBLIC_KEY") {
            let issuer = std::env::var("SETTLERS_JWT_ISSUER").map_err(Error::storage)?;
            if let Ok(server_id) = std::env::var("SETTLERS_SERVER_ID") {
                return Auth::verification_for_server(key.as_bytes(), &issuer, &server_id);
            }
            let game_id = std::env::var("SETTLERS_GAME_ID").map_err(Error::storage)?;
            return Auth::verification_only(key.as_bytes(), &issuer, &game_id);
        }
        let secret = match std::env::var("SETTLERS_JWT_SECRET") {
            Ok(s) => s.into_bytes(),
            Err(_) if self.local => local_key(&self.data_dir)?,
            Err(_) => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "SETTLERS_JWT_SECRET is required in production",
                ));
            }
        };
        Auth::new(secret, self.local)
    }
}

#[cfg(test)]
#[path = "../tests/config/mod.rs"]
mod tests;
fn local_key(dir: &Path) -> Result<Vec<u8>> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(Error::storage)?;
    let path = dir.join("auth.key");
    match std::fs::read(&path) {
        Ok(key) => return Ok(key),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(Error::storage(e)),
        Err(_) => {}
    }
    let mut key = vec![0; 32];
    getrandom::fill(&mut key).map_err(Error::storage)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(&key).map_err(Error::storage)?;
            file.sync_all().map_err(Error::storage)?;
            std::fs::File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(Error::storage)?;
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read(path).map_err(Error::storage)
        }
        Err(e) => Err(Error::storage(e)),
    }
}
