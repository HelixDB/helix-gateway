//! gRPC client wrapper for backend service communication.

use crate::config::GrpcConfig;
use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;
use crate::generated::gateway_proto::RequestType;
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

/// Routes requests to different backends based on request type.
///
/// Supports read replica configurations where reads go to replicas
/// and writes go to the primary. When no read replica is configured,
/// both clients point to the same connection pool.
#[derive(Clone)]
pub struct RoutingClient {
    write_client: ProtoClient,
    read_client: ProtoClient,
    has_read_replica: bool,
}

impl RoutingClient {
    /// Connects to the backend(s) based on configuration.
    ///
    /// If `READ_BACKEND_ADDR` is configured, creates separate connection pools
    /// for reads and writes. Otherwise, both use the primary backend.
    pub async fn connect(config: &GrpcConfig) -> Result<Self> {
        let write_client = ProtoClient::connect(config).await?;

        let (read_client, has_read_replica) = if let Some(ref read_addr) = config.read_backend_addr
        {
            let read_config = GrpcConfig {
                backend_addr: read_addr.clone(),
                ..config.clone()
            };
            (ProtoClient::connect(&read_config).await?, true)
        } else {
            // Share the same client for both read and write
            (write_client.clone(), false)
        };

        Ok(Self {
            write_client,
            read_client,
            has_read_replica,
        })
    }

    /// Returns the appropriate client for the given request type.
    ///
    /// - `RequestType::Write` → write_client (primary)
    /// - `RequestType::Read` → read_client (replica if configured)
    /// - `RequestType::Mcp` → read_client (replica if configured)
    #[inline]
    pub fn client_for(&self, request_type: RequestType) -> &ProtoClient {
        match request_type {
            RequestType::Write => &self.write_client,
            RequestType::Read | RequestType::Mcp => &self.read_client,
        }
    }

    /// Returns the write client directly.
    #[inline]
    pub fn write_client(&self) -> &ProtoClient {
        &self.write_client
    }

    /// Returns the read client directly.
    #[inline]
    pub fn read_client(&self) -> &ProtoClient {
        &self.read_client
    }

    /// Returns true if a separate read replica is configured.
    pub fn has_read_replica(&self) -> bool {
        self.has_read_replica
    }
}

#[cfg(test)]
impl RoutingClient {
    /// Creates a mock routing client for testing purposes.
    pub fn mock() -> Self {
        let client = ProtoClient::mock();
        Self {
            write_client: client.clone(),
            read_client: client,
            has_read_replica: false,
        }
    }

    /// Creates a mock routing client with separate read/write clients.
    pub fn mock_with_replica() -> Self {
        Self {
            write_client: ProtoClient::mock(),
            read_client: ProtoClient::mock(),
            has_read_replica: true,
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

    // RoutingClient tests
    #[tokio::test]
    async fn test_routing_client_mock_no_replica() {
        let client = RoutingClient::mock();
        assert!(!client.has_read_replica());
    }

    #[tokio::test]
    async fn test_routing_client_mock_with_replica() {
        let client = RoutingClient::mock_with_replica();
        assert!(client.has_read_replica());
    }

    #[tokio::test]
    async fn test_routing_client_routes_write_to_write_client() {
        let client = RoutingClient::mock_with_replica();
        let selected = client.client_for(RequestType::Write);
        // Verify it's the write client by comparing pool addresses
        assert!(std::ptr::eq(selected, client.write_client()));
    }

    #[tokio::test]
    async fn test_routing_client_routes_read_to_read_client() {
        let client = RoutingClient::mock_with_replica();
        let selected = client.client_for(RequestType::Read);
        assert!(std::ptr::eq(selected, client.read_client()));
    }

    #[tokio::test]
    async fn test_routing_client_routes_mcp_to_read_client() {
        let client = RoutingClient::mock_with_replica();
        let selected = client.client_for(RequestType::Mcp);
        assert!(std::ptr::eq(selected, client.read_client()));
    }
}
