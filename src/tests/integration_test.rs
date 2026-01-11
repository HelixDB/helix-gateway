//! Integration tests for the Helix Gateway.
//!
//! These tests spin up a mock gRPC backend server and test the full
//! HTTP-to-gRPC translation flow.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use crate::config::{Config, GrpcConfig};
use crate::format::Format;
use crate::gateway::introspection::{DbQuery, Introspection};
use crate::gateway::routes::create_router;
use crate::gateway::state::AppState;
use crate::generated::gateway_proto::backend_service_server::{
    BackendService, BackendServiceServer,
};
use crate::generated::gateway_proto::{
    HealthRequest, HealthResponse, QueryRequest, QueryResponse, RequestType,
};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use http_body_util::BodyExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::{Response, Status, transport::Server};
use tower::ServiceExt;

/// Mock backend service implementation for testing.
#[derive(Debug, Default)]
struct MockBackendService {
    /// If set, the service will return this error for all requests.
    fail_with: Option<Status>,
}

impl MockBackendService {
    fn new() -> Self {
        Self::default()
    }

    fn failing(status: Status) -> Self {
        Self {
            fail_with: Some(status),
        }
    }
}

#[tonic::async_trait]
impl BackendService for MockBackendService {
    async fn query(
        &self,
        request: tonic::Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        if let Some(ref status) = self.fail_with {
            return Err(status.clone());
        }

        let req = request.into_inner();
        Ok(Response::new(QueryResponse {
            data: format!("Query result for: {}", req.query).into_bytes(),
            status: 200,
        }))
    }

    async fn health(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        if let Some(ref status) = self.fail_with {
            return Err(status.clone());
        }

        Ok(Response::new(HealthResponse {
            healthy: true,
            version: "1.0.0-test".to_string(),
        }))
    }
}

/// Starts a mock gRPC server and returns the address and a shutdown signal sender.
async fn start_mock_grpc_server(service: MockBackendService) -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        Server::builder()
            .add_service(BackendServiceServer::new(service))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    shutdown_rx.await.ok();
                },
            )
            .await
            .unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, shutdown_tx)
}

/// Creates a test DbQuery with default values.
fn test_query(request_type: RequestType) -> DbQuery {
    DbQuery {
        request_type,
        parameters: pbjson_types::Struct::default(),
        return_types: pbjson_types::Struct::default(),
        embedding_config: None,
    }
}

/// Creates test queries for integration tests.
fn create_test_queries() -> Introspection {
    let mut queries = HashMap::new();
    // Add test queries used by the integration tests
    queries.insert("query".to_string(), test_query(RequestType::Read));
    queries.insert("get_users".to_string(), test_query(RequestType::Read));
    queries.insert("get_user_by_id".to_string(), test_query(RequestType::Read));
    for i in 0..10 {
        queries.insert(format!("test_query_{}", i), test_query(RequestType::Read));
    }
    Introspection {
        queries,
        schema: HashMap::new(),
    }
}

/// Creates an AppState connected to the given gRPC backend address.
async fn create_test_app_state(backend_addr: &str) -> AppState {
    let grpc_config = GrpcConfig {
        backend_addr: backend_addr.to_string(),
        ..GrpcConfig::default()
    };
    let client = crate::client::ProtoClient::connect(&grpc_config)
        .await
        .expect("Failed to connect to mock backend");

    AppState::new(client)
        .with_config(Config::default())
        .with_format(Format::Json)
        .with_introspection(create_test_queries())
}

/// Helper to make a request to the test router.
async fn make_request(
    router: Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, Bytes) {
    let request = match body {
        Some(b) => Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    };

    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();

    (status, body)
}

// ============================================================================
// Health Endpoint Tests
// ============================================================================

#[tokio::test]
async fn test_health_endpoint_returns_healthy() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let (status, body) = make_request(router, "GET", "/health", None).await;

    assert_eq!(status, StatusCode::OK);
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("\"healthy\":true"));
    assert!(body_str.contains("\"version\":\"1.0.0-test\""));
}

#[tokio::test]
async fn test_health_endpoint_with_backend_error() {
    let service = MockBackendService::failing(Status::unavailable("backend down"));
    let (addr, _shutdown) = start_mock_grpc_server(service).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let (status, body) = make_request(router, "GET", "/health", None).await;

    // gRPC UNAVAILABLE should map to an error response
    assert_ne!(status, StatusCode::OK);
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_str.contains("error"));
}

// ============================================================================
// Query Endpoint Tests
// ============================================================================

#[tokio::test]
async fn test_query_endpoint_success() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let body = r#"{"request_type": 0, "query": "get_users", "parameters": {}}"#;
    let (status, response_body) = make_request(router, "POST", "/query", Some(body)).await;

    assert_eq!(status, StatusCode::OK);
    let body_str = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body_str.contains("\"status\":200"));
}

#[tokio::test]
async fn test_query_endpoint_with_parameters() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let body = r#"{"request_type": 0, "query": "get_user_by_id", "parameters": {"id": "123"}}"#;
    let (status, _) = make_request(router, "POST", "/query", Some(body)).await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_query_endpoint_with_invalid_json() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let body = r#"{"query": invalid_json}"#;
    let (status, response_body) = make_request(router, "POST", "/query", Some(body)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body_str = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body_str.contains("error"));
}

#[tokio::test]
async fn test_query_endpoint_with_empty_body() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let (status, response_body) = make_request(router, "POST", "/query", Some("")).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body_str = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body_str.contains("error"));
}

#[tokio::test]
async fn test_query_endpoint_backend_error() {
    let service = MockBackendService::failing(Status::internal("database error"));
    let (addr, _shutdown) = start_mock_grpc_server(service).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let body = r#"{"request_type": 0, "query": "get_users", "parameters": {}}"#;
    let (status, response_body) = make_request(router, "POST", "/query", Some(body)).await;

    assert_ne!(status, StatusCode::OK);
    let body_str = String::from_utf8(response_body.to_vec()).unwrap();
    assert!(body_str.contains("error"));
}

// ============================================================================
// Router Tests
// ============================================================================

#[tokio::test]
async fn test_unknown_query_returns_404() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    // POST to an unknown query should return 404
    let (status, _) = make_request(router, "POST", "/unknown_query", Some("{}")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_wrong_method_on_query_endpoint() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    // Query endpoint only accepts POST
    let (status, _) = make_request(router, "GET", "/query", None).await;

    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn test_wrong_method_on_health_endpoint() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    // Health endpoint only accepts GET
    let (status, _) = make_request(router, "POST", "/health", Some("{}")).await;

    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

// ============================================================================
// Content-Type Tests
// ============================================================================

#[tokio::test]
async fn test_response_content_type_is_json() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""));

    assert!(content_type.unwrap_or("").contains("application/json"));
}

// ============================================================================
// Concurrent Request Tests
// ============================================================================

#[tokio::test]
async fn test_concurrent_requests() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let state = state.clone();
            tokio::spawn(async move {
                let router = create_router().with_state(state);
                let body = format!(
                    r#"{{"request_type": 0, "query": "test_query_{}", "parameters": {{}}}}"#,
                    i
                );
                let (status, _) = make_request(router, "POST", "/query", Some(&body)).await;
                status
            })
        })
        .collect();

    for handle in handles {
        let status = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
    }
}

// ============================================================================
// Buffer Integration Tests
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::gateway::DbStatus;
use crate::gateway::buffer::Buffer;
use crate::gateway::routes::process_buffer;

/// Dynamic mock backend service that can switch between healthy/unhealthy states.
struct DynamicMockBackendService {
    healthy: Arc<AtomicBool>,
    request_count: Arc<AtomicUsize>,
}

impl DynamicMockBackendService {
    fn new() -> (Self, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let healthy = Arc::new(AtomicBool::new(true));
        let request_count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                healthy: Arc::clone(&healthy),
                request_count: Arc::clone(&request_count),
            },
            healthy,
            request_count,
        )
    }
}

#[tonic::async_trait]
impl BackendService for DynamicMockBackendService {
    async fn query(
        &self,
        request: tonic::Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        self.request_count.fetch_add(1, Ordering::SeqCst);

        if !self.healthy.load(Ordering::SeqCst) {
            return Err(Status::unavailable("database unavailable"));
        }

        let req = request.into_inner();
        Ok(Response::new(QueryResponse {
            data: format!("Query result for: {}", req.query).into_bytes(),
            status: 200,
        }))
    }

    async fn health(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(Status::unavailable("database unavailable"));
        }

        Ok(Response::new(HealthResponse {
            healthy: true,
            version: "1.0.0-test".to_string(),
        }))
    }
}

/// Starts a dynamic mock gRPC server.
async fn start_dynamic_mock_grpc_server(
    service: DynamicMockBackendService,
) -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        Server::builder()
            .add_service(BackendServiceServer::new(service))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    shutdown_rx.await.ok();
                },
            )
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, shutdown_tx)
}

/// Creates AppState with a properly configured buffer and watcher.
async fn create_test_app_state_with_buffer(
    backend_addr: &str,
) -> (
    AppState,
    Arc<Buffer>,
    tokio::sync::watch::Receiver<DbStatus>,
) {
    let grpc_config = GrpcConfig {
        backend_addr: backend_addr.to_string(),
        ..GrpcConfig::default()
    };
    let client = crate::client::ProtoClient::connect(&grpc_config)
        .await
        .expect("Failed to connect to mock backend");

    let (tx, mut rx) = tokio::sync::watch::channel(DbStatus::Healthy);
    rx.mark_changed();

    let buffer = Arc::new(
        Buffer::new()
            .max_size(1000)
            .max_duration(Duration::from_secs(30))
            .set_watcher((tx, rx.clone())),
    );

    let state = AppState::new(client)
        .with_config(Config::default())
        .with_format(Format::Json)
        .with_introspection(create_test_queries())
        .with_buffer(Arc::clone(&buffer));

    (state, buffer, rx)
}

/// Integration test: Buffer fills when DB unavailable, drains on recovery.
#[tokio::test]
async fn test_buffer_fills_and_drains_on_db_recovery() {
    const N: usize = 500;

    // Setup dynamic mock
    let (service, healthy_flag, request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_dynamic_mock_grpc_server(service).await;

    // Setup state with buffer
    let (state, buffer, _watcher_rx) =
        create_test_app_state_with_buffer(&format!("http://{}", addr)).await;

    // Phase 1: DB Healthy - send N requests, all should succeed immediately
    {
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let state = state.clone();
                tokio::spawn(async move {
                    let router = create_router().with_state(state);
                    let body = format!(
                        r#"{{"request_type": 0, "query": "test_query_{}", "parameters": {{}}}}"#,
                        i
                    );
                    make_request(router, "POST", "/query", Some(&body)).await
                })
            })
            .collect();

        for handle in handles {
            let (status, _) = handle.await.unwrap();
            assert_eq!(status, StatusCode::OK, "Phase 1: Request should succeed");
        }

        assert_eq!(buffer.len(), 0, "Phase 1: Buffer should be empty");
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            N,
            "Phase 1: Should have processed N requests"
        );
    }

    // Phase 2: DB Unavailable - send N requests, they should get buffered
    healthy_flag.store(false, Ordering::SeqCst);

    let buffered_handles: Vec<_> = (0..N)
        .map(|i| {
            let state = state.clone();
            tokio::spawn(async move {
                let router = create_router().with_state(state);
                let body = format!(
                    r#"{{"request_type": 0, "query": "test_query_{}", "parameters": {{}}}}"#,
                    i
                );
                make_request(router, "POST", "/query", Some(&body)).await
            })
        })
        .collect();

    // Wait for requests to be buffered (retries take ~700ms due to exponential backoff)
    // Poll until buffer has items or timeout
    let start = std::time::Instant::now();
    while buffer.len() == 0 && start.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify requests are in the buffer
    assert!(
        buffer.len() > 0,
        "Phase 2: Buffer should have pending requests (len={})",
        buffer.len()
    );

    // Phase 3: DB Recovery - make healthy and process buffer
    healthy_flag.store(true, Ordering::SeqCst);

    // Spawn buffer processor
    let buffer_clone = Arc::clone(&buffer);
    let grpc_config = GrpcConfig {
        backend_addr: format!("http://{}", addr),
        ..GrpcConfig::default()
    };
    let client = crate::client::ProtoClient::connect(&grpc_config)
        .await
        .unwrap();

    let processor_handle = tokio::spawn(async move {
        process_buffer(buffer_clone, Duration::from_secs(30), &client, None).await
    });

    // Wait for buffered requests to complete
    for handle in buffered_handles {
        let (status, _) = handle.await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "Phase 3: Buffered request should succeed after recovery"
        );
    }

    // Wait for processor to finish
    let processed = processor_handle.await.unwrap().unwrap();
    assert!(
        processed > 0,
        "Phase 3: Should have processed buffered requests"
    );
    assert_eq!(
        buffer.len(),
        0,
        "Phase 3: Buffer should be empty after drain"
    );

    // Phase 4: Post-Recovery - send N more requests
    {
        let handles: Vec<_> = (0..N)
            .map(|i| {
                let state = state.clone();
                tokio::spawn(async move {
                    let router = create_router().with_state(state);
                    let body = format!(
                        r#"{{"request_type": 0, "query": "test_query_{}", "parameters": {{}}}}"#,
                        i
                    );
                    make_request(router, "POST", "/query", Some(&body)).await
                })
            })
            .collect();

        for handle in handles {
            let (status, _) = handle.await.unwrap();
            assert_eq!(status, StatusCode::OK, "Phase 4: Request should succeed");
        }
    }

    // Final assertions
    assert_eq!(buffer.len(), 0, "Final: Buffer should be empty");
}
