//! HTTP route handlers and application state.
//!
//! This module defines the HTTP API surface of the gateway, translating
//! incoming requests to gRPC calls and formatting responses.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;

use crate::{
    client::ProtoClient,
    config::Config,
    error::GatewayError,
    format::Format,
    generated::gateway_proto::{
        HealthRequest, HealthResponse, McpRequest, McpResponse, QueryRequest, QueryResponse,
    },
};

/// Shared application state available to all request handlers.
#[derive(Clone)]
pub struct AppState {
    config: Config,
    grpc_client: ProtoClient,
    format: Format,
}

impl AppState {
    pub fn new(client: ProtoClient) -> Self {
        Self {
            config: Config::default(),
            grpc_client: client,
            format: Format::default(),
        }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    pub fn with_format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }
}

/// Creates the router with all gateway endpoints.
pub fn create_router() -> Router<AppState> {
    Router::new()
        .route("/query", post(handle_query))
        .route("/health", get(handle_health))
        .route("/mcp", post(handle_mcp))
}

/// Handles `POST /query` requests by forwarding to the backend Query RPC.
pub async fn handle_query(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, GatewayError> {
    let request: QueryRequest = state
        .format
        .deserialize_owned(&body)
        .map_err(GatewayError::from)?;

    let mut client = state.grpc_client.client();
    let response = client.query(request).await?.into_inner();

    let http_response = QueryResponse {
        data: response.data,
        status: response.status,
    };

    let body = state.format.serialize(&http_response)?;

    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            state.format.content_type(),
        )],
        body,
    ))
}

/// Handles `GET /health` requests by checking backend health.
pub async fn handle_health(
    State(mut state): State<AppState>,
) -> Result<Json<HealthResponse>, GatewayError> {
    let response = state
        .grpc_client
        .inner()
        .health(HealthRequest {})
        .await?
        .into_inner();

    Ok(Json(HealthResponse {
        healthy: response.healthy,
        version: response.version,
    }))
}

/// Handles `POST /mcp` requests for Model Context Protocol operations.
pub async fn handle_mcp(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, GatewayError> {
    let request: McpRequest = state.format.deserialize_owned(&body)?;

    let grpc_request = McpRequest {
        method: request.method,
        payload: request.payload,
    };

    let mut client = state.grpc_client.client();
    let response = client.mcp(grpc_request).await?.into_inner();

    let http_response = McpResponse {
        result: response.result,
        error: response.error,
    };

    let body = state.format.serialize(&http_response)?;

    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            state.format.content_type(),
        )],
        body,
    ))
}
