//! Error types and HTTP response conversion.
//!
//! All errors are automatically converted to appropriate HTTP responses
//! with JSON error bodies.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;

/// Gateway error types with automatic HTTP status code mapping.
#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("parse error: {0}")]
    ParseError(#[from] eyre::Error),

    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Backend unavailable")]
    BackendUnavailable,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            GatewayError::ParseError(e) => (StatusCode::BAD_REQUEST, e.to_string()),
            GatewayError::InvalidRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            GatewayError::Grpc(status) => (
                StatusCode::from_u16(status.code() as u16)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                status.message().to_string(),
            ),
            GatewayError::BackendUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Backend unavailable".to_string(),
            ),
        };

        let body = ErrorResponse { error: message };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn test_parse_error_display() {
        let err = GatewayError::ParseError(eyre::eyre!("invalid json"));
        assert!(err.to_string().contains("parse error"));
        assert!(err.to_string().contains("invalid json"));
    }

    #[test]
    fn test_invalid_request_display() {
        let err = GatewayError::InvalidRequest("missing field".to_string());
        assert_eq!(err.to_string(), "Invalid request: missing field");
    }

    #[test]
    fn test_grpc_error_display() {
        let status = tonic::Status::not_found("resource not found");
        let err = GatewayError::Grpc(status);
        assert!(err.to_string().contains("gRPC error"));
    }

    #[test]
    fn test_backend_unavailable_display() {
        let err = GatewayError::BackendUnavailable;
        assert_eq!(err.to_string(), "Backend unavailable");
    }

    #[tokio::test]
    async fn test_parse_error_response_status() {
        let err = GatewayError::ParseError(eyre::eyre!("bad input"));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_invalid_request_response_status() {
        let err = GatewayError::InvalidRequest("invalid".to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_backend_unavailable_response_status() {
        let err = GatewayError::BackendUnavailable;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_error_response_body_format() {
        let err = GatewayError::InvalidRequest("test error".to_string());
        let response = err.into_response();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_str.contains("\"error\""));
        assert!(body_str.contains("test error"));
    }

    #[test]
    fn test_grpc_status_conversion() {
        let status = tonic::Status::permission_denied("not allowed");
        let err = GatewayError::Grpc(status);
        let response = err.into_response();
        // gRPC PERMISSION_DENIED code (7) doesn't map to a valid HTTP status,
        // so it should fall back to INTERNAL_SERVER_ERROR
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_error_from_eyre() {
        let eyre_err = eyre::eyre!("something went wrong");
        let gateway_err: GatewayError = GatewayError::ParseError(eyre_err);
        assert!(gateway_err.to_string().contains("something went wrong"));
    }

    #[test]
    fn test_error_from_tonic_status() {
        let status = tonic::Status::unavailable("service down");
        let gateway_err: GatewayError = status.into();
        matches!(gateway_err, GatewayError::Grpc(_));
    }
}
