//! # Helix Gateway
//!
//! An HTTP-to-gRPC gateway that translates REST API requests into gRPC calls
//! to a backend service.
//!
//! ## Quick Start
//!
//! ```no_run
//! #[tokio::main]
//! async fn main() -> eyre::Result<()> {
//!     helix_gateway::gateway::run().await
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

pub mod client;
pub mod config;
pub mod db;
pub mod error;
pub mod format;
pub mod gateway;
pub mod generated;
pub mod utils;
