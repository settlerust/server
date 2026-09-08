//! Durable authenticated room service and transport adapters.
pub mod auth;
pub mod config;
pub mod error;
pub mod managed_auth;
pub mod metrics;
pub mod model;
pub mod reducer;
#[cfg(feature = "aws")]
pub mod runtime;
pub mod service;
pub mod storage;
pub mod transport;
