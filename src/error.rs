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
