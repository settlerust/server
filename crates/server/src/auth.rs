use crate::error::{Error, Result};
pub use crate::identity::{Identity, validate_name};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use settlers_protocol::{ErrorCode, TokenResponse};
use sha2::Sha256;
use std::{collections::BTreeMap, sync::Arc};

/// Providers authenticate credentials; the server issues the same stable-subject JWT afterward.
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn authenticate(&self, credential: &serde_json::Value) -> Result<Identity>;
}
pub struct GuestProvider;
#[async_trait]
impl IdentityProvider for GuestProvider {
    async fn authenticate(&self, credential: &serde_json::Value) -> Result<Identity> {
        let request: settlers_protocol::GuestRequest =
            serde_json::from_value(credential.clone())
                .map_err(|_| Error::new(ErrorCode::InvalidRequest, "expected a guest name"))?;
        validate_name(&request.name)?;
        Ok(Identity {
            subject: uuid::Uuid::new_v4().to_string(),
            name: request.name,
        })
    }
}
#[derive(Clone)]
pub struct Auth {
    secret: Arc<[u8]>,
    providers: BTreeMap<String, Arc<dyn IdentityProvider>>,
    ttl: u64,
    external: Option<Arc<crate::managed_auth::Verifier>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    iss: String,
    aud: String,
    sub: String,
    name: String,
    iat: u64,
    nbf: u64,
    exp: u64,
}
impl Auth {
    /// Verify RS256 game grants without possessing any signing material.
    pub fn verification_only(public_key: &[u8], issuer: &str, game_id: &str) -> Result<Self> {
        Ok(Self {
            secret: Arc::from([]),
            providers: BTreeMap::new(),
            ttl: 0,
            external: Some(Arc::new(crate::managed_auth::Verifier::new(
                public_key, issuer, game_id,
            )?)),
        })
    }
    pub fn verification_for_server(
        public_key: &[u8],
        issuer: &str,
        server_id: &str,
    ) -> Result<Self> {
        Ok(Self {
            secret: Arc::from([]),
            providers: BTreeMap::new(),
            ttl: 0,
            external: Some(Arc::new(crate::managed_auth::Verifier::for_server(
                public_key, issuer, server_id,
            )?)),
        })
    }
    pub fn external_grant(
        &self,
        token: &str,
        now: u64,
    ) -> Result<Option<crate::managed_auth::Grant>> {
        self.external
            .as_ref()
            .map(|v| v.verify(token, now))
            .transpose()
    }
    pub fn new(secret: Vec<u8>, guests: bool) -> Result<Self> {
        if secret.len() < 32 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "JWT signing key must contain at least 32 bytes",
            ));
        }
        let mut auth = Self {
            secret: secret.into(),
            providers: BTreeMap::new(),
            ttl: 7 * 86400,
            external: None,
        };
        if guests {
            auth.providers
                .insert("guest".into(), Arc::new(GuestProvider));
        }
        Ok(auth)
    }
    pub fn with_provider(mut self, name: String, provider: Arc<dyn IdentityProvider>) -> Self {
        self.providers.insert(name, provider);
        self
    }
    pub async fn login(
        &self,
        provider: &str,
        credential: &serde_json::Value,
        now: u64,
    ) -> Result<TokenResponse> {
        let provider = self.providers.get(provider).ok_or_else(|| {
            Error::new(ErrorCode::Forbidden, "authentication provider is disabled")
        })?;
        let identity = provider.authenticate(credential).await?;
        self.issue(&identity, now)
    }
    pub fn issue(&self, identity: &Identity, now: u64) -> Result<TokenResponse> {
        if self.external.is_some() {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "tokens are issued by the external lobby service",
            ));
        }
        let claims = Claims {
            iss: "server".into(),
            aud: "settlers-v1".into(),
            sub: identity.subject.clone(),
            name: identity.name.clone(),
            iat: now,
            nbf: now,
            exp: now + self.ttl,
        };
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(Error::storage)?);
        let signed = format!("{header}.{payload}");
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).map_err(Error::storage)?;
        mac.update(signed.as_bytes());
        Ok(TokenResponse {
            token: format!(
                "{signed}.{}",
                URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
            ),
            subject: identity.subject.clone(),
            expires_at: claims.exp,
        })
    }
    pub fn verify(&self, token: &str, now: u64) -> Result<Identity> {
        if let Some(verifier) = &self.external {
            return Ok(verifier.verify(token, now)?.identity());
        }
        let invalid = || {
            Error::new(
                ErrorCode::Unauthorized,
                "invalid or expired authentication token",
            )
        };
        if token.len() > 8192 {
            return Err(invalid());
        }
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(invalid());
        }
        let signature = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| invalid())?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).map_err(|_| invalid())?;
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&signature).map_err(|_| invalid())?;
        let header: Header =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        if header.alg != "HS256" || header.typ != "JWT" {
            return Err(invalid());
        }
        let claims: Claims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        if claims.iss != "server"
            || claims.aud != "settlers-v1"
            || claims.exp <= now
            || claims.nbf > now
            || claims.iat > now
            || claims.sub.is_empty()
            || claims.sub.len() > 200
        {
            return Err(invalid());
        }
        Ok(Identity {
            subject: claims.sub,
            name: claims.name,
        })
    }
}
