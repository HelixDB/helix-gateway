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

### Introspection JSON Structure

The `introspect.json` file defines the available queries and their configurations:

```json
{
  "queries": {
    "get_users": {
      "request_type": "READ",
      "parameters": {},
      "return_types": {},
      "embedding_config": null
    },
    "create_user": {
      "request_type": "WRITE",
      "parameters": {
        "name": "string",
        "email": "string"
      },
      "return_types": {
        "id": "string"
      }
    },
    "semantic_search": {
      "request_type": "READ",
      "parameters": {
        "query_text": "string",
        "limit": "number"
      },
      "return_types": {},
      "embedding_config": {
        "provider_config": {
          "provider": "openai",
          "config": {
            "model": "text_embedding_3_large"
          }
        },
        "embedded_variables": ["query_text"]
      }
    }
  },
  "schema": {}
}
```

#### Query Fields

| Field | Type | Description |
|-------|------|-------------|
| `request_type` | `"READ"` \| `"WRITE"` \| `"MCP"` | The type of request |
| `parameters` | object | Parameter definitions for the query |
| `return_types` | object | Return type definitions |
| `embedding_config` | object \| null | Optional embedding configuration for vector search |

#### Embedding Providers

When using vector search, configure `embedding_config` with one of these providers:

**OpenAI**
```json
{
  "provider": "openai",
  "config": {
    "model": "text_embedding_3_large"
  }
}
```
Models: `text_embedding_3_large`, `text_embedding_3_small`, `text_embedding_ada_002`

**Azure OpenAI**
```json
{
  "provider": "azure",
  "config": {
    "model": "text_embedding_3_large",
    "deployment_id": "your-deployment-id"
  }
}
```

**Gemini**
```json
{
  "provider": "gemini",
  "config": {
    "model": "embedding_001",
    "task_type": "RETRIEVAL_DOCUMENT"
  }
}
```

**Local**
```json
{
  "provider": "local",
  "config": {
    "url": "http://localhost:8699/embed"
  }
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
