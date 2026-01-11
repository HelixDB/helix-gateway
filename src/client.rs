//! gRPC client wrapper for backend service communication.

use crate::config::GrpcConfig;
use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;
use eyre::Result;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tonic::transport::{Channel, Endpoint};

/// Default number of connections in the pool.
/// Sized for high throughput: 32 connections × ~100 streams = ~3200 concurrent requests.
const DEFAULT_POOL_SIZE: usize = 32;

/// A wrapper around the generated gRPC client with connection pooling.
///
/// Uses multiple HTTP/2 channels in a round-robin pool to increase concurrent
/// stream capacity. Each HTTP/2 connection has a limited number of concurrent
/// streams (~100-256), so pooling multiple connections allows higher throughput
/// at high concurrency.
///
/// The client is cheaply cloneable and safe to share across tasks.
#[derive(Clone)]
pub struct ProtoClient {
    inner: Arc<ProtoClientInner>,
}

struct ProtoClientInner {
    clients: Vec<BackendServiceClient<Channel>>,
}

impl ProtoClient {
    /// Connects to the gRPC backend with the default pool size.
    ///
    /// Creates multiple HTTP/2 connections for better throughput at high concurrency.
    pub async fn connect(config: &GrpcConfig) -> Result<Self> {
        Self::connect_pooled(config, DEFAULT_POOL_SIZE).await
    }

    /// Connects to the gRPC backend with a specified pool size.
    ///
    /// Each connection is configured with timeouts, keepalive, and HTTP/2 settings
    /// from the provided [`GrpcConfig`].
    ///
    /// # Arguments
    /// * `config` - gRPC configuration
    /// * `pool_size` - Number of connections to create (minimum 1)
    pub async fn connect_pooled(config: &GrpcConfig, pool_size: usize) -> Result<Self> {
        let pool_size = pool_size.max(1);
        let mut clients = Vec::with_capacity(pool_size);

        for _ in 0..pool_size {
            let endpoint = Endpoint::from_shared(config.backend_addr.clone())?
                .connect_timeout(config.connect_timeout)
                .timeout(config.request_timeout)
                .tcp_keepalive(Some(config.tcp_keepalive))
                .http2_keep_alive_interval(config.http2_keepalive_interval)
                .keep_alive_timeout(config.http2_keepalive_timeout)
                .keep_alive_while_idle(true)
                .http2_adaptive_window(config.http2_adaptive_window);

            let channel = endpoint.connect().await?;
            clients.push(BackendServiceClient::new(channel));
        }

        Ok(Self {
            inner: Arc::new(ProtoClientInner { clients }),
        })
    }

    /// Returns a client from the pool using thread-based selection.
    #[inline]
    pub fn client(&self) -> BackendServiceClient<Channel> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::thread::current().id().hash(&mut hasher);
        let idx = hasher.finish() as usize % self.inner.clients.len();
        self.inner.clients[idx].clone()
    }

    /// Returns the number of connections in the pool.
    pub fn pool_size(&self) -> usize {
        self.inner.clients.len()
    }
}

#[cfg(test)]
impl ProtoClient {
    /// Creates a mock client for testing purposes.
    /// The mock will fail on any actual gRPC call.
    pub fn mock() -> Self {
        use tonic::transport::Channel;
        // Create a channel that points to an invalid endpoint
        // This will fail if actually used, but allows us to test code paths
        // that don't make gRPC calls
        let channel = Channel::from_static("http://[::1]:1").connect_lazy();
        Self {
            inner: Arc::new(ProtoClientInner {
                clients: vec![BackendServiceClient::new(channel)],
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_addr(addr: &str) -> GrpcConfig {
        GrpcConfig {
            backend_addr: addr.to_string(),
            ..GrpcConfig::default()
        }
    }

    #[tokio::test]
    async fn test_connect_invalid_uri_scheme() {
        // Invalid URI without scheme should fail
        let config = config_with_addr("invalid-uri-no-scheme");
        let result = ProtoClient::connect(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_empty_address() {
        let config = config_with_addr("");
        let result = ProtoClient::connect(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_malformed_uri() {
        let config = config_with_addr("://malformed");
        let result = ProtoClient::connect(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_unreachable_host() {
        // This should fail to connect but the URI parsing should succeed
        let config = config_with_addr("http://127.0.0.1:1");
        let result = ProtoClient::connect(&config).await;
        // Connection to port 1 should fail (nothing listening there)
        assert!(result.is_err());
    }
}
