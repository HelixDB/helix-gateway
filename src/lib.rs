//! # Helix Gateway
//!
//! An HTTP-to-gRPC gateway that translates REST API requests into gRPC calls
//! to a backend service.
//!
//! ## Quick Start
//!
//! ```no_run
//! use helix_gateway::gateway::GatewayBuilder;
//!
//! #[tokio::main]
//! async fn main() -> eyre::Result<()> {
//!     GatewayBuilder::new()
//!        .run().await
//! }
//! ```
//!
//! ## Configuration
//!
//! The gateway is configured via environment variables:
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `LISTEN_ADDR` | `0.0.0.0:8080` | HTTP server listen address |
//! | `BACKEND_ADDR` | `http://127.0.0.1:50051` | gRPC backend address |
//! | `REQUEST_TIMEOUT_MS` | `30000` | Request timeout in milliseconds |
//!
//! ## Endpoints
//!
//! - `POST /query` - Execute queries on the backend
//! - `GET /health` - Health check endpoint
//! - `POST /mcp` - Model Context Protocol requests

pub(crate) mod client;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod format;
mod generated;
mod utils;

#[cfg(feature = "db")]
pub mod db;
#[cfg(feature = "gateway")]
pub mod gateway;

pub use config::{Config, GrpcConfig};
pub use error::GatewayError;
pub use format::Format;

#[cfg(test)]
mod tests;
