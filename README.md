# Helix Gateway

A lightweight, high-performance HTTP-to-gRPC proxy gateway written in Rust.

Helix Gateway translates HTTP JSON requests into gRPC calls and forwards them to a backend service, serving as an API gateway between HTTP clients and gRPC backends.

## Features

- **HTTP/JSON to gRPC translation** - Seamless protocol bridging
- **High performance** - Built with Tokio async runtime and MiMalloc allocator
- **Fast JSON parsing** - Uses sonic-rs for zero-copy JSON serialization
- **Request tracing** - Structured logging with tracing middleware
- **Configurable timeouts** - Per-request timeout control
- **Health checks** - Built-in health endpoint for monitoring
- **MCP support** - Model Context Protocol endpoint for AI integrations

## Architecture

```
HTTP Client ──▶ Axum Router ──▶ Handler ──▶ gRPC Client ──▶ Backend Service
                                              (Tonic)
```
