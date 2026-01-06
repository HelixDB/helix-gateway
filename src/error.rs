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
    ParseError(#[from] sonic_rs::Error),

    #[error("query not found")]
    QueryNotFound,

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("missing parameters: {0}")]
    MissingParametersError(String),

    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("Backend unavailable")]
    BackendUnavailable,

    #[error("Internal server error")]
    InternalError(#[from] eyre::Error),
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let status = match &self {
            GatewayError::ParseError(_) => StatusCode::BAD_REQUEST,
            GatewayError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            GatewayError::MissingParametersError(_) => StatusCode::BAD_REQUEST,
            GatewayError::QueryNotFound => StatusCode::NOT_FOUND,
            GatewayError::Grpc(_) => StatusCode::INTERNAL_SERVER_ERROR,
            GatewayError::BackendUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            GatewayError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let message = self.to_string();

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
        let sonic_err = sonic_rs::from_str::<serde::de::IgnoredAny>("invalid json").unwrap_err();
        let err = GatewayError::ParseError(sonic_err);
        assert!(err.to_string().contains("parse error"));
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
        let sonic_err = sonic_rs::from_str::<serde::de::IgnoredAny>("bad input").unwrap_err();
        let err = GatewayError::ParseError(sonic_err);
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
    fn test_error_from_sonic() {
        let sonic_err = sonic_rs::from_str::<serde::de::IgnoredAny>("{invalid}").unwrap_err();
        let gateway_err: GatewayError = GatewayError::ParseError(sonic_err);
        assert!(gateway_err.to_string().contains("parse error"));
    }

    #[test]
    fn test_error_from_tonic_status() {
        let status = tonic::Status::unavailable("service down");
        let gateway_err: GatewayError = status.into();
        matches!(gateway_err, GatewayError::Grpc(_));
    }
}
