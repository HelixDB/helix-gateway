# helix-gateway

HTTP gateway and gRPC backend service library for building query-based APIs.

## Features

- `gateway` - HTTP server that translates REST requests to gRPC calls
- `db` - gRPC backend service implementation with handler routing

Both features are enabled by default.

## Gateway Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
helix-gateway = { version = "0.1", features = ["gateway"] }
```

Run the gateway:

```rust
#[tokio::main]
async fn main() -> eyre::Result<()> {
    helix_gateway::gateway::run().await
}
```

The gateway loads query definitions from `introspect.json` and forwards requests to the configured gRPC backend.

## DB Service Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
helix-gateway = { version = "0.1", features = ["db"] }
tonic = "0.12"
futures = "0.3"
inventory = "0.3"
```

### Define your database type

```rust
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct MyDb {
    pub pool: Arc<sqlx::PgPool>,
}
```

### Register handlers

```rust
use helix_gateway::db::router::{Handler, HandlerSubmission, QueryInput};
use futures::future::BoxFuture;

fn get_users(input: QueryInput<MyDb>) -> BoxFuture<'static, eyre::Result<Vec<u8>>> {
    Box::pin(async move {
        let users = sqlx::query("SELECT * FROM users")
            .fetch_all(&*input.db.pool)
            .await?;
        Ok(serde_json::to_vec(&users)?)
    })
}

fn create_user(input: QueryInput<MyDb>) -> BoxFuture<'static, eyre::Result<Vec<u8>>> {
    Box::pin(async move {
        // Handle write operation
        Ok(vec![])
    })
}

// Register read handler
inventory::submit! {
    HandlerSubmission(Handler::new("get_users", get_users))
}

// Register write handler
inventory::submit! {
    HandlerSubmission(Handler::new("create_user", create_user).is_write())
}
```

### Start the gRPC server

```rust
use helix_gateway::db::{DbService, BackendServiceServer};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let db = MyDb::default();
    let service = DbService::new(db);

    Server::builder()
        .add_service(BackendServiceServer::new(service))
        .serve("0.0.0.0:50051".parse()?)
        .await?;

    Ok(())
}
```

## Configuration

All settings are configured via environment variables with sensible defaults.

### Gateway

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8080` | HTTP server bind address |
| `REQUEST_TIMEOUT_MS` | `30000` | HTTP request timeout |

### gRPC Client

| Variable | Default | Description |
|----------|---------|-------------|
| `BACKEND_ADDR` | `http://127.0.0.1:50051` | gRPC backend address |
| `GRPC_CONNECT_TIMEOUT_MS` | `5000` | Connection timeout |
| `GRPC_REQUEST_TIMEOUT_MS` | `30000` | Request timeout |
| `GRPC_TCP_KEEPALIVE_SECS` | `60` | TCP keepalive interval |
| `GRPC_HTTP2_KEEPALIVE_INTERVAL_SECS` | `60` | HTTP/2 ping interval |
| `GRPC_HTTP2_KEEPALIVE_TIMEOUT_SECS` | `20` | HTTP/2 ping timeout |
| `GRPC_HTTP2_ADAPTIVE_WINDOW` | `true` | Adaptive flow control |
