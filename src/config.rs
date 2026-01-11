//! Gateway configuration loaded from environment variables.

use std::net::SocketAddr;
use std::time::Duration;

/// gRPC client configuration for backend connections.
///
/// Controls connection behavior, timeouts, and HTTP/2 settings.
///
/// # Environment Variables
///
/// - `GRPC_CONNECT_TIMEOUT_MS` - Connection establishment timeout (default: `5000`)
/// - `GRPC_REQUEST_TIMEOUT_MS` - Per-request timeout (default: `30000`)
/// - `GRPC_TCP_KEEPALIVE_SECS` - TCP keepalive interval (default: `60`)
/// - `GRPC_HTTP2_KEEPALIVE_INTERVAL_SECS` - HTTP/2 ping interval (default: `60`)
/// - `GRPC_HTTP2_KEEPALIVE_TIMEOUT_SECS` - HTTP/2 ping timeout (default: `20`)
/// - `GRPC_HTTP2_ADAPTIVE_WINDOW` - Enable adaptive flow control (default: `true`)
#[derive(Clone, Debug)]
pub struct GrpcConfig {
    /// Backend gRPC address
    pub(crate) backend_addr: String,
    /// Connection establishment timeout
    pub(crate) connect_timeout: Duration,
    /// Per-request timeout
    pub(crate) request_timeout: Duration,
    /// TCP keepalive interval
    pub(crate) tcp_keepalive: Duration,
    /// HTTP/2 keepalive ping interval
    pub(crate) http2_keepalive_interval: Duration,
    /// HTTP/2 keepalive ping timeout
    pub(crate) http2_keepalive_timeout: Duration,
    /// Enable HTTP/2 adaptive flow control window
    pub(crate) http2_adaptive_window: bool,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            backend_addr: "http://127.0.0.1:50051".into(),
            connect_timeout: Duration::from_millis(5_000),
            request_timeout: Duration::from_millis(30_000),
            tcp_keepalive: Duration::from_secs(60),
            http2_keepalive_interval: Duration::from_secs(60),
            http2_keepalive_timeout: Duration::from_secs(20),
            http2_adaptive_window: true,
        }
    }
}

impl GrpcConfig {
    /// Loads gRPC configuration from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        Self {
            backend_addr: std::env::var("BACKEND_ADDR")
                .unwrap_or_else(|_| "http://127.0.0.1:50051".into()),
            connect_timeout: Duration::from_millis(
                std::env::var("GRPC_CONNECT_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5_000),
            ),
            request_timeout: Duration::from_millis(
                std::env::var("GRPC_REQUEST_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30_000),
            ),
            tcp_keepalive: Duration::from_secs(
                std::env::var("GRPC_TCP_KEEPALIVE_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
            ),
            http2_keepalive_interval: Duration::from_secs(
                std::env::var("GRPC_HTTP2_KEEPALIVE_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60),
            ),
            http2_keepalive_timeout: Duration::from_secs(
                std::env::var("GRPC_HTTP2_KEEPALIVE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(20),
            ),
            http2_adaptive_window: std::env::var("GRPC_HTTP2_ADAPTIVE_WINDOW")
                .ok()
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
        }
    }

    /// Sets the backend address.
    pub fn with_backend_addr(mut self, addr: impl Into<String>) -> Self {
        self.backend_addr = addr.into();
        self
    }
}

/// Server configuration.
///
/// Load from environment with [`Config::from_env`], or use [`Config::default`]
/// for development defaults.
///
/// # Environment Variables
///
/// - `LISTEN_ADDR` - HTTP server bind address (default: `0.0.0.0:8080`)
/// - `REQUEST_TIMEOUT_MS` - HTTP request timeout in milliseconds (default: `30000`)
///
/// See [`GrpcConfig`] for gRPC-specific environment variables.
#[derive(Clone)]
pub struct Config {
    pub(crate) listen_addr: SocketAddr,
    pub(crate) request_timeout_ms: u64,
    pub(crate) grpc: GrpcConfig,
    pub(crate) max_qps: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8080"
                .parse()
                .expect("Invalid default listen address"),
            request_timeout_ms: 30_000,
            grpc: GrpcConfig::default(),
            max_qps: 1000,
        }
    }
}

impl Config {
    /// Loads configuration from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        Self {
            listen_addr: std::env::var("LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8080".into())
                .parse()
                .expect("Invalid LISTEN_ADDR"),
            request_timeout_ms: std::env::var("REQUEST_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30_000),
            grpc: GrpcConfig::from_env(),
            max_qps: std::env::var("MAX_QPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_listen_addr() {
        let config = Config::default();
        assert_eq!(config.listen_addr.to_string(), "0.0.0.0:8080");
    }

    #[test]
    fn test_config_default_grpc_backend_addr() {
        let config = Config::default();
        assert_eq!(config.grpc.backend_addr, "http://127.0.0.1:50051");
    }

    #[test]
    fn test_config_default_timeout() {
        let config = Config::default();
        assert_eq!(config.request_timeout_ms, 30_000);
    }

    #[test]
    fn test_config_clone() {
        let config = Config::default();
        let cloned = config.clone();
        assert_eq!(config.listen_addr, cloned.listen_addr);
        assert_eq!(config.grpc.backend_addr, cloned.grpc.backend_addr);
        assert_eq!(config.request_timeout_ms, cloned.request_timeout_ms);
    }

    #[test]
    fn test_config_listen_addr_is_socket_addr() {
        let config = Config::default();
        assert_eq!(config.listen_addr.port(), 8080);
        assert!(config.listen_addr.ip().is_unspecified());
    }

    #[test]
    fn test_grpc_config_defaults() {
        let grpc = GrpcConfig::default();
        assert_eq!(grpc.connect_timeout, Duration::from_millis(5_000));
        assert_eq!(grpc.request_timeout, Duration::from_millis(30_000));
        assert_eq!(grpc.tcp_keepalive, Duration::from_secs(60));
        assert_eq!(grpc.http2_keepalive_interval, Duration::from_secs(60));
        assert_eq!(grpc.http2_keepalive_timeout, Duration::from_secs(20));
        assert!(grpc.http2_adaptive_window);
    }
}
