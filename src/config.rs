use std::net::SocketAddr;

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
