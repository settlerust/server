//! Optional external token verification for an independently operated lobby service.
use crate::{
    auth::Identity,
    error::{Error, Result},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use settlers_protocol::{Command, ErrorCode};

pub struct Verifier {
    key: DecodingKey,
    validation: Validation,
    game_id: Option<String>,
    server_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Player,
    Spectator,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub name: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    pub game_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    pub role: Role,
}
impl Verifier {
    pub fn new(public_key: &[u8], issuer: &str, game_id: &str) -> Result<Self> {
        if issuer.is_empty() || uuid::Uuid::parse_str(game_id).is_err() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "external issuer and assigned game UUID are required",
            ));
        }
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&["settlers-v1"]);
        // Use the caller's clock, including deterministic tests, for all time checks.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.leeway = 0;
        Ok(Self {
            key: DecodingKey::from_rsa_pem(public_key).map_err(Error::storage)?,
            validation,
            game_id: Some(game_id.into()),
            server_id: None,
        })
    }
    /// Accept independently scoped games assigned to this shared server instance.
    pub fn for_server(public_key: &[u8], issuer: &str, server_id: &str) -> Result<Self> {
        let mut verifier = Self::new(public_key, issuer, server_id)?;
        verifier.game_id = None;
        verifier.server_id = Some(server_id.into());
        Ok(verifier)
    }
    pub fn verify(&self, token: &str, now: u64) -> Result<Grant> {
        let invalid = || Error::new(ErrorCode::Unauthorized, "invalid or expired external token");
        if token.len() > 8192 {
            return Err(invalid());
        }
        let decoded = decode::<Grant>(token, &self.key, &self.validation).map_err(|_| invalid())?;
        let claims = decoded.claims;
        if decoded.header.typ.as_deref() != Some("JWT")
            || claims.exp <= now
            || claims.nbf > now
            || claims.iat > now
            || claims.exp.saturating_sub(claims.iat) > 3600
            || claims.sub.is_empty()
            || claims.sub.len() > 200
            || uuid::Uuid::parse_str(&claims.game_id).is_err()
            || self
                .game_id
                .as_ref()
                .is_some_and(|id| id != &claims.game_id)
            || self
                .server_id
                .as_ref()
                .is_some_and(|id| claims.server_id.as_ref() != Some(id))
            || crate::auth::validate_name(&claims.name).is_err()
        {
            return Err(invalid());
        }
        Ok(claims)
    }
}
impl Grant {
    pub fn identity(&self) -> Identity {
        Identity {
            subject: self.sub.clone(),
            name: self.name.clone(),
        }
    }
    pub fn authorize_path(&self, path: &str, post: bool) -> Result<()> {
        if path.starts_with("/v1/auth/") || (path == "/v1/games" && post && self.role != Role::Host)
        {
            return Err(denied());
        }
        if let Some(rest) = path.strip_prefix("/v1/games/")
            && rest.split('/').next() != Some(self.game_id.as_str())
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn authorize_command(&self, command: &Command) -> Result<()> {
        if self.role == Role::Player
            && matches!(
                command,
                Command::Join {
                    spectator: true,
                    ..
                }
            )
        {
            return Err(denied());
        }
        if matches!(command, Command::Configure { .. }) {
            return Err(denied());
        }
        if self.role == Role::Spectator
            && !matches!(
                command,
                Command::Join {
                    spectator: true,
                    ..
                }
            )
        {
            return Err(denied());
        }
        Ok(())
    }
}
fn denied() -> Error {
    Error::new(
        ErrorCode::Forbidden,
        "the external lobby has not granted this action",
    )
}
