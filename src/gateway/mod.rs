// Copyright 2026 HelixDB
// SPDX-License-Identifier: Apache-2.0

//! Gateway server initialization and lifecycle management.
//!
use crate::{
    Format, config,
    gateway::{
        buffer::Buffer,
        introspection::Introspection,
        routes::{create_router, process_buffer},
        state::AppState,
    },
};
use axum::{Router, http::StatusCode};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};
use tracing::info;

pub mod buffer;
mod embeddings;
pub mod introspection;
mod mcp;
pub mod routes;
pub mod state;

#[doc(hidden)]
pub use crate::client::ProtoClient;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DbStatus {
    #[default]
    Healthy,
    Unhealthy,
}

pub struct GatewayBuilder {
    config: config::Config,
    format: Format,
    state: Option<AppState>,
}

impl GatewayBuilder {
    /// Creates a new `GatewayBuilder` instance.
    ///
    /// Uses configuration based on environment variables and default format (Json).
    pub fn new() -> Self {
        Self {
            config: config::Config::from_env(),
            format: Format::default(),
            state: None,
        }
    }

    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    pub fn config(mut self, config: config::Config) -> Self {
        self.config = config;
        self
    }

    pub fn state(mut self, state: AppState) -> Self {
        self.state = Some(state);
        self
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
    pub async fn run(self) -> eyre::Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("helix_gateway=info".parse()?),
            )
            .init();

        let grpc_client = ProtoClient::connect(&self.config.grpc).await?;
        info!(
            "Connected to backend at {} (connect_timeout={:?}, request_timeout={:?})",
            self.config.grpc.backend_addr,
            self.config.grpc.connect_timeout,
            self.config.grpc.request_timeout
        );

        let introspection = load_queries("introspect.json")?;
        info!(
            "Loaded {} queries from introspect.json",
            introspection.queries.len()
        );

        // Initialize embedding client pool based on required providers
        let required_providers = introspection.required_providers();
        let embedding_pool = embeddings::EmbeddingClientPool::new(&required_providers)?;
        info!(
            "Initialized embedding clients: openai={}, azure={}, gemini={}, local={}",
            required_providers.needs_openai,
            required_providers.needs_azure,
            required_providers.needs_gemini,
            required_providers.local_urls.len()
        );

        let (tx, mut rx) = tokio::sync::watch::channel(DbStatus::default());
        rx.mark_unchanged(); // mark unchanged means the first check on unavailable database request will trigger buffer
        let request_buffer = Arc::new(
            Buffer::new()
                .max_duration(Duration::from_millis(1000))
                .max_size(1000)
                .set_watcher((tx, rx.clone())),
        );

        let state = AppState::new(grpc_client.clone())
            .with_config(self.config.clone())
            .with_format(self.format)
            .with_introspection(introspection)
            .with_embedding_pool(embedding_pool)
            .with_buffer(Arc::clone(&request_buffer));

        let app: Router = create_router()
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                Duration::from_millis(self.config.request_timeout_ms),
            ))
            .layer(TraceLayer::new_for_http())
            .with_state(state.clone());

        let request_timeout = Duration::from_millis(state.config.request_timeout_ms);
        let limiter = state
            .rate_limiter
            .as_ref()
            .map(|limiter| Arc::clone(&limiter));

        let _ = tokio::spawn(async move {
            loop {
                let _ = rx.changed().await;
                if *rx.borrow() == DbStatus::Unhealthy {
                    continue;
                }

                // process pending requests
                let _ = process_buffer(
                    Arc::clone(&request_buffer),
                    request_timeout,
                    &grpc_client,
                    limiter.clone(),
                )
                .await;
            }
        });

        let listener = TcpListener::bind(self.config.listen_addr).await?;
        info!("Listening on {}", self.config.listen_addr);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

fn load_queries(path: &str) -> eyre::Result<Introspection> {
    let file = std::fs::read_to_string(path)?;
    let map: Introspection = sonic_rs::from_str(&file)?;
    Ok(map)
}
