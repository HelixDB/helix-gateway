//! Gateway configuration loaded from environment variables.

use std::net::SocketAddr;

/// Server configuration.
///
/// Load from environment with [`Config::from_env`], or use [`Config::default`]
/// for development defaults.
///
/// # Environment Variables
///
/// - `LISTEN_ADDR` - HTTP server bind address (default: `0.0.0.0:8080`)
/// - `BACKEND_ADDR` - gRPC backend URL (default: `http://127.0.0.1:50051`)
/// - `REQUEST_TIMEOUT_MS` - Request timeout in milliseconds (default: `30000`)
#[derive(Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub backend_addr: String,
    pub request_timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8080"
                .parse()
                .expect("Invalid default listen address"),
            backend_addr: "http://127.0.0.1:50051".into(),
            request_timeout_ms: 30_000,
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
            backend_addr: std::env::var("BACKEND_ADDR")
                .unwrap_or_else(|_| "http://127.0.0.1:50051".into()),
            request_timeout_ms: std::env::var("REQUEST_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30_000),
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
    fn test_config_default_backend_addr() {
        let config = Config::default();
        assert_eq!(config.backend_addr, "http://127.0.0.1:50051");
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
        assert_eq!(config.backend_addr, cloned.backend_addr);
        assert_eq!(config.request_timeout_ms, cloned.request_timeout_ms);
    }

    #[test]
    fn test_config_listen_addr_is_socket_addr() {
        let config = Config::default();
        assert_eq!(config.listen_addr.port(), 8080);
        assert!(config.listen_addr.ip().is_unspecified());
    }
}
