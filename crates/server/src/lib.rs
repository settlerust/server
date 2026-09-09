//! Durable authenticated room service and transport adapters.
#[cfg(not(target_arch = "wasm32"))]
pub mod auth;
#[cfg(not(target_arch = "wasm32"))]
pub mod config;
pub mod error;
#[cfg(not(target_arch = "wasm32"))]
pub mod managed_auth;
#[cfg(not(target_arch = "wasm32"))]
pub mod metrics;
pub mod model;
pub mod reducer;
#[cfg(all(feature = "aws", not(target_arch = "wasm32")))]
pub mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod service;
#[cfg(not(target_arch = "wasm32"))]
pub mod storage;
#[cfg(not(target_arch = "wasm32"))]
pub mod transport;

pub mod browser;
pub mod identity;
#[cfg(target_arch = "wasm32")]
pub mod auth {
    pub use crate::identity::{Identity, validate_name};
}
