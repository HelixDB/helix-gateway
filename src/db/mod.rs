// Copyright 2026 HelixDB
// SPDX-License-Identifier: Apache-2.0

//! Database service implementation for the gRPC backend.
//!
//! This module implements the `BackendService` trait, providing handlers
//! for Read, Write, Health, and MCP requests with async route dispatch.

use crate::db::router::{QueryInput, Router};
use tonic::{Request, Response, Status};

pub(crate) mod router;

pub use crate::generated::gateway_proto::{
    HealthRequest, HealthResponse, QueryRequest, QueryResponse, RequestType,
    backend_service_server::{BackendService, BackendServiceServer},
};

/// Database service that implements the gRPC BackendService.
///
/// Routes are dispatched by matching the query string (for Read/Write)
/// or method name (for MCP) against registered handlers.
#[derive(Default)]
pub struct DbService<DB: Send + Sync + Default + 'static> {
    db: DB,
    router: Router<DB>,
}

impl<DB: Send + Sync + Default + 'static> DbService<DB> {
    /// Creates a new DbService instance with no routes.
    pub fn new(db: DB) -> Self {
        let mut new = Self::default();
        new.db = db;
        new.router = Router::new();
        new
    }
}

#[tonic::async_trait]
impl<DB: Send + Sync + Clone + Default + 'static> BackendService for DbService<DB> {
    /// Handles read requests by dispatching to registered handlers.
    ///
    /// The query string is used as the routing key. If no handler is found,
    /// returns Status::not_found().
    async fn query(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        let req = request.into_inner();
        let query_key = &req.query;

        let routes = match req.request_type() {
            RequestType::Read => &self.router.read_routes(),
            RequestType::Write => &self.router.write_routes(),
            RequestType::Mcp => &self.router.mcp_routes(),
        };

        let handler = routes
            .get(query_key.as_str())
            .ok_or_else(|| Status::not_found(query_key))?;

        let request = QueryInput::new(req, self.db.clone());

        let result = handler(request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let response = QueryResponse {
            data: result,
            status: 200,
        };
        Ok(Response::new(response))
    }

    /// Handles health check requests.
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }
}
