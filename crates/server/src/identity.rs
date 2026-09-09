//! Platform-independent participant identity and display-name validation.
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use settlers_protocol::ErrorCode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub subject: String,
    pub name: String,
}
pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "name must be 1–80 bytes without control characters",
        ));
    }
    Ok(())
}
