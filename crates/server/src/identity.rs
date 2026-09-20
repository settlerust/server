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
/// Legacy hosted identities have no privacy revision. Never expose their raw name.
pub fn legacy_display_name(subject: &str, name: &str) -> String {
    if !["google:", "github:", "email:", "guest:"]
        .iter()
        .any(|prefix| subject.starts_with(prefix))
    {
        return name.to_owned();
    }
    let mut words = name.split_whitespace();
    let first = words.next().unwrap_or("Player");
    match words.next_back().and_then(|last| last.chars().next()) {
        Some(last) => format!("{first} {}", last.to_uppercase()),
        None => first.to_owned(),
    }
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
