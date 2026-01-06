//! The idea here is that all endpoints are registered as a query
//!
//! Write handlers are registered with is_write = true
//! MCP handlers are registered with is_mcp = true
//! Write MCP handlers are registered with is_write = true and is_mcp = true
//! If any handler is embedding, it is registered with is_embedding = true
//!
//! Embeddings get handled in the gateway via an async request
//!
//! The MCP connection pooling and tracking get handled by the gateway
//!
//! Also all introspection queries will get handled by the gateway (will check cache first before calling from db)

use futures::future::BoxFuture;
use std::{collections::HashMap, sync::Arc};

use crate::generated::gateway_proto::QueryRequest;

pub type Response<R> = eyre::Result<R>;
pub(crate) type BasicHandlerFn<DB> = fn(QueryInput<DB>) -> BoxFuture<'static, Response<Vec<u8>>>;
pub(crate) type HandlerFn<DB> =
    Arc<dyn Fn(QueryInput<DB>) -> BoxFuture<'static, Response<Vec<u8>>> + Send + Sync>;
pub(crate) type Routes<DB> = HashMap<String, HandlerFn<DB>>;

pub struct QueryInput<DB> {
    pub request: QueryRequest,
    pub db: DB,
}
impl<DB> QueryInput<DB> {
    pub(crate) fn new(request: QueryRequest, db: DB) -> Self {
        Self { request, db }
    }
}

#[derive(Clone, Debug)]
pub struct Handler<DB: Send + Sync> {
    pub name: &'static str,
    pub func: BasicHandlerFn<DB>,
    pub is_write: bool,
    pub is_mcp: bool,
    pub is_embedding: bool,
}

impl<DB: Send + Sync> Handler<DB> {
    pub const fn new(name: &'static str, func: BasicHandlerFn<DB>) -> Self {
        Self {
            name,
            func,
            is_write: false,
            is_mcp: false,
            is_embedding: false,
        }
    }

    pub const fn is_write(mut self) -> Self {
        self.is_write = true;
        self
    }

    pub const fn is_mcp(mut self) -> Self {
        self.is_mcp = true;
        self
    }

    pub const fn is_embedding(mut self) -> Self {
        self.is_embedding = true;
        self
    }
}
#[derive(Clone, Debug)]
pub struct HandlerSubmission<DB: Send + Sync>(pub Handler<DB>);

impl<DB: Send + Sync + 'static> inventory::Collect for HandlerSubmission<DB> {
    #[inline]
    fn registry() -> &'static inventory::Registry {
        static REGISTRY: inventory::Registry = inventory::Registry::new();
        &REGISTRY
    }
}

#[derive(Default)]
pub(crate) struct Router<DB: Send + Sync + Default> {
    read_routes: Option<Routes<DB>>,
    write_routes: Option<Routes<DB>>,
    mcp_routes: Option<Routes<DB>>,
}

impl<DB: Send + Sync + Default + 'static> Router<DB> {
    /// Creates a new DbService instance with no routes.
    pub(crate) fn new() -> Self {
        let (read_routes, write_routes, mcp_routes): (
            HashMap<String, HandlerFn<DB>>,
            HashMap<String, HandlerFn<DB>>,
            HashMap<String, HandlerFn<DB>>,
        ) = inventory::iter::<HandlerSubmission<DB>>.into_iter().fold(
            (HashMap::new(), HashMap::new(), HashMap::new()),
            |(mut routes, mut writes, mut mcp), submission| {
                println!(
                    "Processing POST submission for handler: {} (is_write: {})",
                    submission.0.name, submission.0.is_write
                );
                let handler = &submission.0;
                let func: HandlerFn<DB> = Arc::new(handler.func);

                if handler.is_write {
                    writes.insert(handler.name.to_string(), func);
                } else if handler.is_mcp {
                    mcp.insert(handler.name.to_string(), func);
                } else {
                    routes.insert(handler.name.to_string(), func);
                }
                (routes, writes, mcp)
            },
        );
        Router {
            read_routes: Some(read_routes),
            write_routes: Some(write_routes),
            mcp_routes: Some(mcp_routes),
        }
    }

    pub(crate) fn read_routes(&self) -> &HashMap<String, HandlerFn<DB>> {
        self.read_routes.as_ref().unwrap()
    }

    pub(crate) fn write_routes(&self) -> &HashMap<String, HandlerFn<DB>> {
        self.write_routes.as_ref().unwrap()
    }

    pub(crate) fn mcp_routes(&self) -> &HashMap<String, HandlerFn<DB>> {
        self.mcp_routes.as_ref().unwrap()
    }
}

// TODO: handler to run the functions DONE
// TODO: handle embeddings before calling db DONE
// TODO: handle schema caching and introspection DOING
// TODO: handle mcp connections in gateway DOING
