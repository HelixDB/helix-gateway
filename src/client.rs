//! gRPC client wrapper for backend service communication.

use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;
use eyre::Result;
use tonic::transport::Channel;

/// A wrapper around the generated gRPC client that simplifies connection management.
///
/// The client is cheaply cloneable (backed by a channel) and safe to share across tasks.
#[derive(Clone)]
pub struct ProtoClient {
    client: BackendServiceClient<Channel>,
}

impl ProtoClient {
    /// Connects to the gRPC backend at the given address.
    pub async fn connect(addr: &str) -> Result<Self> {
        let channel = Channel::from_shared(addr.to_string())?.connect().await?;

        Ok(Self {
            client: BackendServiceClient::new(channel),
        })
    }

    /// Returns a mutable reference to the underlying client.
    ///
    /// Use this when you need a single operation and want to avoid cloning.
    pub fn inner(&mut self) -> &mut BackendServiceClient<Channel> {
        &mut self.client
    }

    /// Returns a cloned client for use in concurrent operations.
    ///
    /// The clone is cheap as the underlying channel is shared.
    pub fn client(&self) -> BackendServiceClient<Channel> {
        self.client.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_invalid_uri_scheme() {
        // Invalid URI without scheme should fail
        let result = ProtoClient::connect("invalid-uri-no-scheme").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_empty_address() {
        let result = ProtoClient::connect("").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_malformed_uri() {
        let result = ProtoClient::connect("://malformed").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_unreachable_host() {
        // This should fail to connect but the URI parsing should succeed
        let result = ProtoClient::connect("http://127.0.0.1:1").await;
        // Connection to port 1 should fail (nothing listening there)
        assert!(result.is_err());
    }
}
