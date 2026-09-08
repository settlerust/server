use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use settlers_protocol::{ApiError, ErrorCode};
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0:?}")]
pub struct Error(pub ApiError);
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(ApiError {
            code,
            message: message.into(),
            rule: None,
            current_version: None,
            winner: None,
        })
    }
    pub fn conflict(version: u64) -> Self {
        let mut e = Self::new(
            ErrorCode::Conflict,
            "refresh the snapshot and retry with its version",
        );
        e.0.current_version = Some(version);
        e
    }
    pub fn storage(e: impl std::fmt::Display) -> Self {
        tracing::error!(error=%e,"storage operation failed");
        Self::new(
            ErrorCode::StorageUnavailable,
            "durable storage is temporarily unavailable",
        )
    }
}
impl From<settlers_engine::RuleError> for Error {
    fn from(rule: settlers_engine::RuleError) -> Self {
        Self(ApiError {
            code: ErrorCode::RuleViolation,
            message: rule.to_string(),
            rule: Some(rule),
            current_version: None,
            winner: None,
        })
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict | ErrorCode::TradeTaken | ErrorCode::TradeUnavailable => {
                StatusCode::CONFLICT
            }
            ErrorCode::StorageUnavailable | ErrorCode::Backpressure => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(self.0)).into_response()
    }
}
pub type Result<T> = std::result::Result<T, Error>;
