//! HTTP route handlers.
//!
//! This module defines the HTTP API surface of the gateway, translating
//! incoming requests to gRPC calls and formatting responses.

use crate::{
    client::ProtoClient,
    error::GatewayError,
    gateway::{DbStatus, buffer::Buffer, embeddings, state::AppState},
    generated::gateway_proto::{HealthRequest, HealthResponse, QueryRequest, QueryResponse},
    utils::MaybeOwned,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use bytes::Bytes;
use eyre::eyre;
use governor::RateLimiter;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use pbjson_types::Struct;
use std::{sync::Arc, time::Duration};
use tokio_retry::{Retry, strategy::ExponentialBackoff};
use tonic::Code;
pub type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// Creates the router with all gateway endpoints.
pub fn create_router() -> Router<AppState> {
    Router::new()
        .route("/{query}", post(handle_query))
        .route("/health", get(handle_health))
}

/// Handles `POST /{query}` requests by forwarding to the backend Query RPC.
pub async fn handle_query(
    Path(query): Path<String>,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, GatewayError> {
    if let Some(rate_limiter) = &state.rate_limiter {
        if rate_limiter.check().is_err() {
            return Err(GatewayError::RateLimited);
        }
    }

    let db_query = state
        .introspection
        .queries
        .get(&query)
        .ok_or(())
        .map_err(|_| GatewayError::QueryNotFound)?;

    let parameters: MaybeOwned<Struct> = state.format.deserialize(&body)?;

    // Handle embeddings using batched API for better throughput
    let mut embeddings = None;
    if let Some(ref embedding_config) = db_query.embedding_config {
        // Collect all texts that need embedding
        let texts_to_embed: Vec<(&str, &str)> = embedding_config
            .embedded_variables
            .iter()
            .filter_map(|key| {
                if let Some(value) = parameters.fields.get(key)
                    && let Some(pbjson_types::value::Kind::StringValue(s)) = &value.kind
                {
                    Some((key.as_str(), s.as_str()))
                } else {
                    None
                }
            })
            .collect();

        if !texts_to_embed.is_empty() {
            let embedding_results =
                embeddings::embed_batch(&state.embedding_pool, embedding_config, texts_to_embed)
                    .await?;

            embeddings = Some(Struct::default());
            for (key, embedding) in embedding_results {
                insert_embedding(&mut embeddings, &key, embedding);
            }
        }
    }

    let request = QueryRequest {
        request_type: db_query.request_type as i32,
        query,
        parameters: Some(parameters.into_owned()),
        embeddings,
    };

    let retry_strategy = ExponentialBackoff::from_millis(100)
        .max_delay(Duration::from_secs(1))
        .take(3);
    let response = match Retry::spawn(retry_strategy, async || {
        let mut client = state.grpc_client.client();
        client.query(request.clone()).await
    })
    .await
    {
        Ok(response) => {
            state
                .buffer
                .update_watcher(DbStatus::Healthy)
                .map_err(|e| GatewayError::InternalError(eyre!(e)))?;

            Ok(response.into_inner())
        }
        Err(err) => {
            if matches!(err.code(), Code::Unavailable | Code::DeadlineExceeded) {
                state
                    .buffer
                    .update_watcher(DbStatus::Unhealthy)
                    .map_err(|e| GatewayError::InternalError(eyre!(e)))?;
                state
                    .buffer
                    .enqueue(request)?
                    .await
                    .map_err(|e| GatewayError::InternalError(eyre!(e)))?
            } else {
                Err(GatewayError::BackendUnavailable) // TODO
            }
        }
    }?;

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

/// Inserts the embedding vector into the parameters struct.
fn insert_embedding(embeddings: &mut Option<Struct>, key: &str, embedding: Vec<f64>) {
    debug_assert!(embeddings.is_some());
    use pbjson_types::{ListValue, Value, value::Kind};

    let list = ListValue {
        values: embedding
            .into_iter()
            .map(|v| Value {
                kind: Some(Kind::NumberValue(v)),
            })
            .collect(),
    };

    embeddings.as_mut().unwrap().fields.insert(
        key.to_string(),
        Value {
            kind: Some(Kind::ListValue(list)),
        },
    );
}

/// Handles `GET /health` requests by checking backend health.
pub async fn handle_health(
    State(state): State<AppState>,
) -> Result<Json<HealthResponse>, GatewayError> {
    let mut client = state.grpc_client.client();
    let response = client.health(HealthRequest {}).await?.into_inner();

    Ok(Json(HealthResponse {
        healthy: response.healthy,
        version: response.version,
    }))
}

pub async fn process_buffer(
    buffer: Arc<Buffer>,
    timeout: Duration,
    grpc_client: &ProtoClient,
    rate_limiter: Option<Arc<Limiter>>,
) -> eyre::Result<usize> {
    let mut processed = 0;

    while let Some(req) = buffer.dequeue() {
        if let Some(ref limiter) = rate_limiter {
            limiter.until_ready().await;
        }

        // Skip if client disconnected
        if req.is_cancelled() {
            continue;
        }

        // Check timeout
        if req.enqueued_at.elapsed() > timeout {
            let _ = req.respond(Err(GatewayError::RequestTimeout));
            continue;
        }

        // Process and send response
        let retry_strategy = ExponentialBackoff::from_millis(100)
            .max_delay(Duration::from_secs(1))
            .take(3);
        let response = match Retry::spawn(retry_strategy, async || {
            let mut client = grpc_client.client();
            client.query(req.request.clone()).await
        })
        .await
        {
            Ok(response) => {
                buffer
                    .update_watcher(DbStatus::Healthy)
                    .map_err(|e| GatewayError::InternalError(eyre!(e)))?;

                Ok(response.into_inner())
            }
            Err(err) => {
                if matches!(err.code(), Code::Unavailable | Code::DeadlineExceeded) {
                    buffer
                        .update_watcher(DbStatus::Unhealthy)
                        .map_err(|e| GatewayError::InternalError(eyre!(e)))?;

                    buffer
                        .enqueue(req.request.clone())
                        .unwrap()
                        .try_recv()
                        .unwrap()
                } else {
                    Err(GatewayError::BackendUnavailable) // TODO
                }
            }
        };

        let _ = req.respond(response);
        processed += 1;
    }

    Ok(processed)
}
#[cfg(test)]
mod tests {
    use pbjson_types::{Value, value::Kind};

    use crate::{Config, Format};

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
        assert_eq!(config.grpc.backend_addr, "http://127.0.0.1:50051");
    }

    #[test]
    fn test_format_default_is_json() {
        let format = Format::default();
        assert_eq!(format.content_type(), "application/json");
    }

    #[test]
    fn test_query_request_deserialization() {
        let json = r#"{"request_type": 0, "query": "get_users", "parameters": {}}"#;
        let format = Format::Json;
        let result: Result<QueryRequest, _> = format.deserialize_owned(json.as_bytes());
        assert!(result.is_ok());
        let request = result.unwrap();
        assert_eq!(request.query, "get_users");
    }

    #[test]
    fn test_query_request_with_parameters() {
        let json =
            r#"{"request_type": 1, "query": "create_user", "parameters": {"name": "Alice"}}"#;
        let format = Format::Json;
        let result: Result<QueryRequest, _> = format.deserialize_owned(json.as_bytes());
        assert!(result.is_ok());
        let request = result.unwrap();
        assert_eq!(request.query, "create_user");
        assert_eq!(
            request.parameters.as_ref().unwrap().fields.get("name"),
            Some(&Value {
                kind: Some(Kind::StringValue("Alice".to_string())),
            })
        );
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

    // process_buffer tests
    mod process_buffer_tests {
        use super::*;
        use crate::client::ProtoClient;
        use crate::gateway::buffer::Buffer;
        use std::sync::Arc;
        use std::time::Duration;

        fn make_test_request(query: &str) -> QueryRequest {
            QueryRequest {
                request_type: 0,
                query: query.to_string(),
                parameters: None,
                embeddings: None,
            }
        }

        #[tokio::test]
        async fn test_process_empty_buffer() {
            let buffer = Arc::new(Buffer::new());
            let client = ProtoClient::mock();

            let result = process_buffer(buffer, Duration::from_secs(30), &client, None).await;

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 0);
        }

        #[tokio::test]
        async fn test_process_buffer_skips_cancelled() {
            let buffer = Arc::new(Buffer::new());
            let client = ProtoClient::mock();

            // Enqueue a request then drop the receiver (simulating client disconnect)
            let rx = buffer.enqueue(make_test_request("test")).unwrap();
            drop(rx);

            // Process should skip the cancelled request without making gRPC call
            let result =
                process_buffer(buffer.clone(), Duration::from_secs(30), &client, None).await;

            assert!(result.is_ok());
            // Cancelled requests are skipped, not counted as processed
            assert_eq!(result.unwrap(), 0);
        }

        #[tokio::test]
        async fn test_process_buffer_timeout_handling() {
            let buffer = Arc::new(Buffer::new());
            let client = ProtoClient::mock();

            // Enqueue a request
            let rx = buffer.enqueue(make_test_request("test")).unwrap();

            // Process with zero timeout - request should be considered expired
            let result = process_buffer(buffer.clone(), Duration::ZERO, &client, None).await;

            assert!(result.is_ok());
            // Timed out requests are not counted as processed
            assert_eq!(result.unwrap(), 0);

            // The receiver should get a timeout error
            let response = rx.await;
            assert!(response.is_ok());
            let inner = response.unwrap();
            assert!(matches!(inner, Err(crate::GatewayError::RequestTimeout)));
        }

        #[tokio::test]
        async fn test_process_buffer_multiple_cancelled() {
            let buffer = Arc::new(Buffer::new());
            let client = ProtoClient::mock();

            // Enqueue multiple requests and cancel them all
            for i in 0..5 {
                let rx = buffer
                    .enqueue(make_test_request(&format!("test_{}", i)))
                    .unwrap();
                drop(rx);
            }

            assert_eq!(buffer.len(), 5);

            let result =
                process_buffer(buffer.clone(), Duration::from_secs(30), &client, None).await;

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 0);
        }
    }
}
