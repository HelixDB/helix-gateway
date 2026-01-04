use std::time::Duration;

use crate::{
    client::ProtoClient,
    config,
    format::Format,
    gateway::routes::{create_router, AppState},
};
use axum::{http::StatusCode, Router};
use tokio::net::TcpListener;
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tracing::info;

pub mod routes;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

    let state = AppState::new(grpc_client)
        .with_config(config.clone())
        .with_format(Format::Json);

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
