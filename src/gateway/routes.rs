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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_router_has_query_route() {
        let router: Router<AppState> = create_router();
        // Router is created successfully with the expected type
        let _ = router;
    }

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert_eq!(config.listen_addr.port(), 8080);
        assert_eq!(config.backend_addr, "http://127.0.0.1:50051");
    }

    #[test]
    fn test_format_default_is_json() {
        let format = Format::default();
        assert_eq!(format.content_type(), "application/json");
    }

    #[test]
    fn test_query_request_deserialization() {
        let json = r#"{"query": "get_users", "parameters": {}}"#;
        let format = Format::Json;
        let result: Result<QueryRequest, _> = format.deserialize_owned(json.as_bytes());
        assert!(result.is_ok());
        let request = result.unwrap();
        assert_eq!(request.query, "get_users");
    }

    #[test]
    fn test_mcp_request_deserialization() {
        // payload is bytes (Vec<u8>) so we need to pass base64 or array representation
        let json = r#"{"method": "test_method", "payload": []}"#;
        let format = Format::Json;
        let result: Result<McpRequest, _> = format.deserialize_owned(json.as_bytes());
        assert!(result.is_ok());
        let request = result.unwrap();
        assert_eq!(request.method, "test_method");
    }

    #[test]
    fn test_query_response_serialization() {
        let response = QueryResponse {
            data: b"test data".to_vec(),
            status: 200,
        };
        let format = Format::Json;
        let result = format.serialize(&response);
        assert!(result.is_ok());
        let json_bytes = result.unwrap();
        let json = String::from_utf8(json_bytes.to_vec()).unwrap();
        assert!(json.contains("\"status\":200"));
    }

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            healthy: true,
            version: "1.0.0".to_string(),
        };
        let format = Format::Json;
        let result = format.serialize(&response);
        assert!(result.is_ok());
        let json_bytes = result.unwrap();
        let json = String::from_utf8(json_bytes.to_vec()).unwrap();
        assert!(json.contains("\"healthy\":true"));
        assert!(json.contains("\"version\":\"1.0.0\""));
    }

    #[test]
    fn test_mcp_response_serialization() {
        let response = McpResponse {
            result: b"result data".to_vec(),
            error: "".to_string(),
        };
        let format = Format::Json;
        let result = format.serialize(&response);
        assert!(result.is_ok());
    }

    #[test]
    fn test_malformed_json_deserialization() {
        let json = r#"{"query": invalid}"#;
        let format = Format::Json;
        let result: Result<QueryRequest, _> = format.deserialize_owned(json.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_completely_invalid_json() {
        let json = r#"not json at all"#;
        let format = Format::Json;
        let result: Result<QueryRequest, _> = format.deserialize_owned(json.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_json_object() {
        let json = r#"{}"#;
        let format = Format::Json;
        let result: Result<QueryRequest, _> = format.deserialize_owned(json.as_bytes());
        // Proto3 defaults empty strings and maps, so this may succeed
        // Just verify it doesn't panic
        let _ = result;
    }
}
