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
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub aws: bool,
    pub local: bool,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let local = match std::env::var("SETTLERS_MODE").as_deref() {
            Ok("production") => false,
            Ok("local") | Err(_) => true,
            _ => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "SETTLERS_MODE must be local or production",
                ));
            }
        };
        let bind = std::env::var("SETTLERS_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()
            .map_err(|_| Error::new(ErrorCode::InvalidRequest, "invalid SETTLERS_BIND"))?;
        let aws = std::env::var("SETTLERS_STORAGE").unwrap_or_else(|_| "local".into());
        if aws != "aws" && aws != "local" {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "SETTLERS_STORAGE must be local or aws",
            ));
        }
        Ok(Self {
            bind,
            data_dir: std::env::var_os("SETTLERS_DATA_DIR")
                .map_or_else(|| PathBuf::from("data"), PathBuf::from),
            aws: aws == "aws",
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
