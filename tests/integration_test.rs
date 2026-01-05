//! Integration tests for the Helix Gateway.
//!
//! These tests spin up a mock gRPC backend server and test the full
//! HTTP-to-gRPC translation flow.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use bytes::Bytes;
use helix_gateway::config::Config;
use helix_gateway::format::Format;
use helix_gateway::gateway::routes::{create_router, AppState};
use helix_gateway::generated::gateway_proto::backend_service_server::{
    BackendService, BackendServiceServer,
};
use helix_gateway::generated::gateway_proto::{
    HealthRequest, HealthResponse, QueryRequest, QueryResponse,
};
use http_body_util::BodyExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::{transport::Server, Response, Status};
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
async fn start_mock_grpc_server(
    service: MockBackendService,
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

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    (addr, shutdown_tx)
}

/// Creates an AppState connected to the given gRPC backend address.
async fn create_test_app_state(backend_addr: &str) -> AppState {
    let client = helix_gateway::client::ProtoClient::connect(backend_addr)
        .await
        .expect("Failed to connect to mock backend");

    AppState::new(client)
        .with_config(Config::default())
        .with_format(Format::Json)
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
async fn test_unknown_route_returns_404() {
    let (addr, _shutdown) = start_mock_grpc_server(MockBackendService::new()).await;
    let state = create_test_app_state(&format!("http://{}", addr)).await;
    let router = create_router().with_state(state);

    let (status, _) = make_request(router, "GET", "/unknown", None).await;

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
                let body = format!(r#"{{"request_type": 0, "query": "test_query_{}", "parameters": {{}}}}"#, i);
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
