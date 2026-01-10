use std::{num::NonZeroU32, sync::Arc};

use governor::{Quota, RateLimiter};

use crate::{
    Config, Format,
    client::ProtoClient,
    gateway::{
        buffer::Buffer, embeddings::EmbeddingClientPool, introspection::Introspection,
        routes::Limiter,
    },
};

/// Shared application state available to all request handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub grpc_client: ProtoClient,
    pub format: Format,
    pub introspection: Introspection,
    pub embedding_pool: EmbeddingClientPool,
    pub buffer: Arc<Buffer>,
    pub rate_limiter: Option<Arc<Limiter>>,
}

impl AppState {
    pub fn new(client: ProtoClient) -> Self {
        Self {
            config: Config::default(),
            grpc_client: client,
            format: Format::default(),
            introspection: Introspection::default(),
            embedding_pool: EmbeddingClientPool::default(),
            buffer: Arc::new(Buffer::default()),
            rate_limiter: None,
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

    pub fn with_introspection(mut self, introspection: Introspection) -> Self {
        self.introspection = introspection;
        self
    }

    pub fn with_embedding_pool(mut self, pool: EmbeddingClientPool) -> Self {
        self.embedding_pool = pool;
        self
    }

    pub fn with_rate_limiter(mut self, max_qps: u32) -> Self {
        let quota = Quota::per_second(NonZeroU32::new(max_qps).unwrap());
        self.rate_limiter = Some(Arc::new(RateLimiter::direct(quota)));
        self
    }

    pub fn with_buffer(mut self, buffer: Arc<Buffer>) -> Self {
        self.buffer = buffer;
        self
    }
}
