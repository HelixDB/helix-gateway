//! gRPC client wrapper for backend service communication.

use crate::config::GrpcConfig;
use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;
use eyre::Result;
use tonic::transport::{Channel, Endpoint};

/// A wrapper around the generated gRPC client with connection pooling via HTTP/2.
///
/// The client is cheaply cloneable (backed by a channel) and safe to share across tasks.
/// HTTP/2 multiplexes many concurrent requests over a single connection efficiently.
///
/// The channel is configured with:
/// - Connection and request timeouts
/// - TCP keepalive for connection health
/// - HTTP/2 keepalive pings for idle connection detection
/// - Adaptive flow control for optimal throughput
#[derive(Clone)]
pub struct ProtoClient {
    client: BackendServiceClient<Channel>,
}

impl ProtoClient {
    /// Connects to the gRPC backend with the given configuration.
    ///
    /// The channel is configured with timeouts, keepalive, and HTTP/2 settings
    /// from the provided [`GrpcConfig`].
    pub async fn connect(config: &GrpcConfig) -> Result<Self> {
        let endpoint = Endpoint::from_shared(config.backend_addr.clone())?
            // Connection establishment timeout
            .connect_timeout(config.connect_timeout)
            // Per-request timeout
            .timeout(config.request_timeout)
            // TCP keepalive for connection health
            .tcp_keepalive(Some(config.tcp_keepalive))
            // HTTP/2 keepalive ping interval
            .http2_keep_alive_interval(config.http2_keepalive_interval)
            // HTTP/2 keepalive ping timeout
            .keep_alive_timeout(config.http2_keepalive_timeout)
            // Send keepalive pings even when idle
            .keep_alive_while_idle(true)
            // Adaptive flow control window for better throughput
            .http2_adaptive_window(config.http2_adaptive_window);

        let channel = endpoint.connect().await?;

        Ok(Self {
            client: BackendServiceClient::new(channel),
        })
    }

    /// Returns a cloned client for use in request handlers.
    ///
    /// The clone is cheap as the underlying channel is shared. This is the
    /// recommended way to get a client for making gRPC calls.
    pub fn client(&self) -> BackendServiceClient<Channel> {
        self.client.clone()
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
            client: BackendServiceClient::new(channel),
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
