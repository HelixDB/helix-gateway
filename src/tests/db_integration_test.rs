//! Integration tests for the DbService implementation.
//!
//! These tests verify the handler registration system, routing logic,
//! and gRPC service behavior of the db module.

use std::net::SocketAddr;
use std::time::Duration;

use crate::db::router::{Handler, HandlerSubmission, QueryInput, Response};
use crate::db::{BackendServiceServer, DbService};
use crate::generated::gateway_proto::backend_service_server::BackendService;
use crate::generated::gateway_proto::{HealthRequest, QueryRequest, RequestType};
use futures::future::BoxFuture;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::Request;
use tonic::transport::Server;

// ============================================================================
// Test Database Type
// ============================================================================

/// A simple test database type used for testing.
#[derive(Default, Clone)]
struct TestDb;

// ============================================================================
// Test Handlers
// ============================================================================

/// A read handler that returns the query as the result.
fn test_read_handler(input: QueryInput<TestDb>) -> BoxFuture<'static, Response<Vec<u8>>> {
    Box::pin(async move {
        let result = format!("Read result for: {}", input.request.query);
        Ok(result.into_bytes())
    })
}

/// A write handler that acknowledges the write operation.
fn test_write_handler(input: QueryInput<TestDb>) -> BoxFuture<'static, Response<Vec<u8>>> {
    Box::pin(async move {
        let result = format!("Write completed for: {}", input.request.query);
        Ok(result.into_bytes())
    })
}

/// An MCP handler that returns method information.
fn test_mcp_handler(input: QueryInput<TestDb>) -> BoxFuture<'static, Response<Vec<u8>>> {
    Box::pin(async move {
        let result = format!("MCP result for: {}", input.request.query);
        Ok(result.into_bytes())
    })
}

/// A handler that returns an error.
fn test_error_handler(_input: QueryInput<TestDb>) -> BoxFuture<'static, Response<Vec<u8>>> {
    Box::pin(async move { Err(eyre::eyre!("Handler error occurred")) })
}

// ============================================================================
// Handler Registration
// ============================================================================

inventory::submit! {
    HandlerSubmission::<TestDb>(Handler::new("test_read", test_read_handler))
}

inventory::submit! {
    HandlerSubmission::<TestDb>(Handler::new("test_write", test_write_handler).is_write())
}

inventory::submit! {
    HandlerSubmission::<TestDb>(Handler::new("test_mcp", test_mcp_handler).is_mcp())
}

inventory::submit! {
    HandlerSubmission::<TestDb>(Handler::new("test_error", test_error_handler))
}

// ============================================================================
// Health Endpoint Tests
// ============================================================================

#[tokio::test]
async fn test_db_service_health_returns_healthy() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(HealthRequest {});

    let response = service.health(request).await.unwrap();
    let health = response.into_inner();

    assert!(health.healthy);
}

#[tokio::test]
async fn test_db_service_health_returns_version() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(HealthRequest {});

    let response = service.health(request).await.unwrap();
    let health = response.into_inner();

    // Version should be the package version from Cargo.toml
    assert!(!health.version.is_empty());
    assert!(health.version.starts_with("0.")); // Assuming version starts with 0.x
}

// ============================================================================
// Read Query Tests
// ============================================================================

#[tokio::test]
async fn test_db_service_read_query_success() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(QueryRequest {
        request_type: RequestType::Read as i32,
        query: "test_read".to_string(),
        parameters: None,
        embeddings: None,
    });

    let response = service.query(request).await.unwrap();
    let query_response = response.into_inner();

    assert_eq!(query_response.status, 200);
    let result = String::from_utf8(query_response.data).unwrap();
    assert!(result.contains("Read result for: test_read"));
}

#[tokio::test]
async fn test_db_service_read_query_unknown_handler() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(QueryRequest {
        request_type: RequestType::Read as i32,
        query: "nonexistent_handler".to_string(),
        parameters: None,
        embeddings: None,
    });

    let result = service.query(request).await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);
}

// ============================================================================
// Write Query Tests
// ============================================================================

#[tokio::test]
async fn test_db_service_write_query_success() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(QueryRequest {
        request_type: RequestType::Write as i32,
        query: "test_write".to_string(),
        parameters: None,
        embeddings: None,
    });

    let response = service.query(request).await.unwrap();
    let query_response = response.into_inner();

    assert_eq!(query_response.status, 200);
    let result = String::from_utf8(query_response.data).unwrap();
    assert!(result.contains("Write completed for: test_write"));
}

#[tokio::test]
async fn test_db_service_write_handler_not_in_read_routes() {
    let service = DbService::<TestDb>::new(TestDb::default());

    // Try to call write handler via read request type - should fail
    let request = Request::new(QueryRequest {
        request_type: RequestType::Read as i32,
        query: "test_write".to_string(),
        parameters: None,
        embeddings: None,
    });

    let result = service.query(request).await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);
}

// ============================================================================
// MCP Query Tests
// ============================================================================

#[tokio::test]
async fn test_db_service_mcp_query_success() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(QueryRequest {
        request_type: RequestType::Mcp as i32,
        query: "test_mcp".to_string(),
        parameters: None,
        embeddings: None,
    });

    let response = service.query(request).await.unwrap();
    let query_response = response.into_inner();

    assert_eq!(query_response.status, 200);
    let result = String::from_utf8(query_response.data).unwrap();
    assert!(result.contains("MCP result for: test_mcp"));
}

#[tokio::test]
async fn test_db_service_mcp_handler_not_in_read_routes() {
    let service = DbService::<TestDb>::new(TestDb::default());

    // Try to call MCP handler via read request type - should fail
    let request = Request::new(QueryRequest {
        request_type: RequestType::Read as i32,
        query: "test_mcp".to_string(),
        parameters: None,
        embeddings: None,
    });

    let result = service.query(request).await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::NotFound);
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_db_service_handler_error_returns_internal_status() {
    let service = DbService::<TestDb>::new(TestDb::default());
    let request = Request::new(QueryRequest {
        request_type: RequestType::Read as i32,
        query: "test_error".to_string(),
        parameters: None,
        embeddings: None,
    });

    let result = service.query(request).await;

    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Internal);
    assert!(status.message().contains("Handler error occurred"));
}

// ============================================================================
// gRPC Server Integration Tests
// ============================================================================

/// Starts a DbService as a gRPC server and returns the address and shutdown sender.
async fn start_db_grpc_server() -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let service = DbService::<TestDb>::new(TestDb::default());

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

#[tokio::test]
async fn test_db_service_grpc_server_health() {
    use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;

    let (addr, _shutdown) = start_db_grpc_server().await;

    let mut client = BackendServiceClient::connect(format!("http://{}", addr))
        .await
        .unwrap();

    let response = client.health(HealthRequest {}).await.unwrap();
    let health = response.into_inner();

    assert!(health.healthy);
}

#[tokio::test]
async fn test_db_service_grpc_server_query() {
    use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;

    let (addr, _shutdown) = start_db_grpc_server().await;

    let mut client = BackendServiceClient::connect(format!("http://{}", addr))
        .await
        .unwrap();

    let response = client
        .query(&QueryRequest {
            request_type: RequestType::Read as i32,
            query: "test_read".to_string(),
            parameters: None,
            embeddings: None,
        })
        .await
        .unwrap();

    let query_response = response.into_inner();
    assert_eq!(query_response.status, 200);
}
