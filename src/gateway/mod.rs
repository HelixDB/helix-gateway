//! Gateway server initialization and lifecycle management.
//!
use crate::{
    client::ProtoClient,
    config,
    format::Format,
    gateway::{
        introspection::Introspection,
        routes::{AppState, create_router},
    },
};
use axum::{Router, http::StatusCode};
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tracing::info;

pub mod embeddings;
pub mod introspection;
pub mod mcp;
pub mod routes;

fn load_queries(path: &str) -> eyre::Result<Introspection> {
    let file = std::fs::read_to_string(path)?;
    let map: Introspection = sonic_rs::from_str(&file)?;
    Ok(map)
}

/// Starts the gateway server.
///
/// This function:
/// 1. Initializes tracing with environment-based log filtering
/// 2. Loads configuration from environment variables
/// 3. Connects to the gRPC backend service
/// 4. Configures middleware (timeout, request tracing)
/// 5. Binds to the configured address and serves HTTP requests
///
/// # Errors
///
/// Returns an error if:
/// - The gRPC backend connection fails
/// - The TCP listener cannot bind to the configured address
/// - The server encounters a fatal error while running
pub async fn run() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("helix_gateway=info".parse()?),
        )
        .init();

    let config = config::Config::from_env();

    let grpc_client = ProtoClient::connect(&config.backend_addr).await?;
    info!("Connected to backend at {}", config.backend_addr);

    let introspection = load_queries("introspect.json")?;
    info!(
        "Loaded {} queries from introspect.json",
        introspection.queries.len()
    );

    let state = AppState::new(grpc_client)
        .with_config(config.clone())
        .with_format(Format::Json)
        .with_introspection(introspection);

    let app: Router = create_router()
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_millis(config.request_timeout_ms),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = TcpListener::bind(config.listen_addr).await?;
    info!("Listening on {}", config.listen_addr);

    axum::serve(listener, app).await?;
    Ok(())
}
