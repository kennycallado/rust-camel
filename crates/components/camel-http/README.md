# camel-component-http

> HTTP client and server component for rust-camel

## Overview

The HTTP component provides HTTP client (producer) and HTTP server (consumer) capabilities for rust-camel. Built on `reqwest` for clients and `axum` for servers, it enables REST API integration, webhook handling, and HTTP-based messaging.

## Features

- **HTTP Server (Consumer)**: Listen for incoming HTTP requests
- **HTTP Client (Producer)**: Make outgoing HTTP requests
- **HTTPS Support**: Secure connections with `https` scheme
- **Configurable Timeouts**: Connect, response, and pool idle timeouts
- **Connection Pooling**: Shared `reqwest::Client` with configurable pool settings
- **SSRF Protection**: Optional private IP blocking
- **Streaming**: Direct stream-to-HTTP piping without materialization
- **Native Request Streaming**: Incoming bodies arrive as `Body::Stream`, no RAM materialization
- **Native Response Streaming**: `Body::Stream` responses use chunked transfer encoding automatically
- **Header Mapping**: Automatic header forwarding
- **Status Code Handling**: Configurable success ranges

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-http = "0.2"
```

## URI Format

```
http://host:port/path[?options]
https://host:port/path[?options]
```

## Consumer Options (Server)

| Option | Default | Description |
|--------|---------|-------------|
| Host/path from URI | - | e.g., `http://0.0.0.0:8080/api` |
| `maxRequestBody` | `2097152` (2 MB) | If request `Content-Length` exceeds this value, responds 413 before opening the stream. Chunked uploads without `Content-Length` are not limited at the consumer level. |

## Producer Options (Client)

| Option | Default | Description |
|--------|---------|-------------|
| `httpMethod` | Auto | HTTP method (GET, POST, etc.) |
| `throwExceptionOnFailure` | `true` | Throw on non-2xx responses |
| `okStatusCodeRange` | `200-299` | Success status code range |
| `responseTimeout` | Global default | Response timeout (ms) |
| `allowPrivateIps` | Global default | Allow requests to private IPs |
| `blockedHosts` | Global default | Comma-separated blocked hosts |
| `maxBodySize` | Global default | Max response body size (bytes) |

> **Connection-level settings** (`connect_timeout_ms`, `pool_max_idle_per_host`, `pool_idle_timeout_ms`, `follow_redirects`) are configured globally in `Camel.toml` and cannot be overridden per URI. See [Global Configuration](#global-configuration).

## Usage

### HTTP Server (Consumer)

```rust
use camel_builder::RouteBuilder;
use camel_component_http::HttpComponent;
use camel_core::CamelContext;

let mut ctx = CamelContext::new();
ctx.register_component("http", Box::new(HttpComponent::new()));

// Simple API endpoint
let route = RouteBuilder::from("http://0.0.0.0:8080/hello")
    .process(|ex| async move {
        let mut ex = ex;
        ex.input.body = camel_api::Body::Text("Hello, World!".to_string());
        Ok(ex)
    })
    .build()?;
```

### HTTP Client (Producer)

```rust
// GET request
let route = RouteBuilder::from("timer:tick?period=60000")
    .to("http://api.example.com/data?allowPrivateIps=false")
    .log("Response received", camel_processor::LogLevel::Info)
    .build()?;

// POST request (body becomes request body)
let route = RouteBuilder::from("direct:submit")
    .to("http://api.example.com/submit?httpMethod=POST")
    .build()?;
```

### Dynamic HTTP Method

```rust
// Method from header
let route = RouteBuilder::from("direct:api")
    .set_header("CamelHttpMethod", Value::String("DELETE".into()))
    .to("http://api.example.com/resource")
    .build()?;
```

### Dynamic URL

```rust
// URL from header
let route = RouteBuilder::from("direct:proxy")
    .set_header("CamelHttpUri", Value::String("http://backend.service/api".into()))
    .to("http://localhost/dummy")  // Base URL, overridden by header
    .build()?;
```

### HTTPS

```rust
let route = RouteBuilder::from("timer:secure")
    .to("https://secure.api.com/endpoint")
    .build()?;
```

## Exchange Headers

### Request Headers (Consumer)

| Header | Description |
|--------|-------------|
| `CamelHttpMethod` | HTTP method (GET, POST, etc.) |
| `CamelHttpPath` | Request path |
| `CamelHttpQuery` | Query string |
| All HTTP headers | Forwarded from request |

### Response Headers (Producer)

| Header | Description |
|--------|-------------|
| `CamelHttpResponseCode` | HTTP status code |
| `CamelHttpResponseText` | Status text |
| All response headers | Forwarded from response |

### Request Control Headers (Producer)

| Header | Description |
|--------|-------------|
| `CamelHttpUri` | Override target URL |
| `CamelHttpPath` | Append to base URL path |
| `CamelHttpQuery` | Append query string |
| `CamelHttpMethod` | Override HTTP method |

## Example: REST API Server

```rust
use camel_builder::RouteBuilder;
use camel_component_http::{HttpComponent, HttpsComponent};
use camel_core::CamelContext;
use camel_api::Body;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("http", Box::new(HttpComponent::new()));

    // GET /api/users
    ctx.add_route(
        RouteBuilder::from("http://0.0.0.0:8080/api/users")
            .process(|ex| async move {
                let mut ex = ex;
                ex.input.body = Body::Text(r#"[{"id":1,"name":"Alice"}]"#.to_string());
                ex.input.set_header("Content-Type", serde_json::Value::String("application/json".into()));
                Ok(ex)
            })
            .build()?
    ).await?;

    // POST /api/users
    ctx.add_route(
        RouteBuilder::from("http://0.0.0.0:8080/api/users")
            .filter(|ex| ex.input.header("CamelHttpMethod").and_then(|v| v.as_str()) == Some("POST"))
                .process(|ex| async move {
                    // Create user from request body
                    Ok(ex)
                })
                .set_body(Body::Text(r#"{"status":"created"}"#))
            .end_filter()
            .build()?
    ).await?;

    ctx.start().await?;
    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;

    Ok(())
}
```

## Example: HTTP Client with Error Handling

```rust
let route = RouteBuilder::from("direct:api-call")
    .to("http://api.service.com/endpoint?throwExceptionOnFailure=true&responseTimeout=5000")
    .build()?;

// With custom error handling
let route = RouteBuilder::from("direct:resilient")
    .error_handler(ErrorHandlerConfig::log_only())
    .to("http://api.service.com/endpoint?throwExceptionOnFailure=false")
    .process(|ex| async move {
        let status = ex.input.header("CamelHttpResponseCode")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if status >= 400 {
            // Handle error response
        }
        Ok(ex)
    })
    .build()?;
```

## Global Configuration

Configure default HTTP settings in `Camel.toml` that apply to all HTTP endpoints:

```toml
[default.components.http]
connect_timeout_ms = 5000           # Connection timeout (default: 5000)
pool_max_idle_per_host = 100        # Max idle connections per host (default: 100)
pool_idle_timeout_ms = 90000        # Idle connection timeout (default: 90000)
follow_redirects = false            # Follow HTTP redirects (default: false)
response_timeout_ms = 30000         # Response timeout (default: 30000)
max_body_size = 10485760            # Max response body size, 10MB (default: 10MB)
max_request_body = 2097152          # Max request body for server, 2MB (default: 2MB)
allow_private_ips = false           # Allow requests to private IPs (default: false)
blocked_hosts = []                  # Blocked host list (default: empty)
```

URI parameters override **request-level** defaults (`responseTimeout`, `allowPrivateIps`, `blockedHosts`, `maxBodySize`). Connection-level settings (`connect_timeout_ms`, `pool_*`, `follow_redirects`) are baked into the shared connection pool and cannot be overridden per URI.

### Profile-Specific Configuration

```toml
[default.components.http]
connect_timeout_ms = 30000
pool_max_idle_per_host = 100

[production.components.http]
connect_timeout_ms = 5000   # Faster fail in production
pool_idle_timeout_ms = 60000
allow_private_ips = false

[development.components.http]
connect_timeout_ms = 60000  # More lenient in dev
allow_private_ips = true    # Allow internal services in dev
```

## SSRF Protection

By default, the HTTP client blocks requests to private IP addresses for security. To allow:

```rust
.to("http://internal.service/api?allowPrivateIps=true")
```

To block specific hosts:

```rust
.to("http://api.example.com?blockedHosts=localhost,127.0.0.1,internal.local")
```

## Streaming & Memory Management

The HTTP component supports native streaming for both producer and consumer.

### Producer (Client) Streaming

The HTTP producer supports streaming request bodies directly without materializing them in memory. Stream bodies are piped to reqwest using `wrap_stream()`.

Memory limits apply when materialization is required (default: 10MB).

### Consumer (Server) Streaming

**Request Bodies:** Incoming HTTP request bodies arrive as `Body::Stream` in the Exchange, with no RAM materialization by default. The `Content-Length` header (if present) populates `StreamMetadata.size_hint`, and `Content-Type` populates `StreamMetadata.content_type`.

**413 Protection:** If the `Content-Length` header exceeds `maxRequestBody`, the server responds with HTTP 413 before opening the stream. Chunked uploads without a `Content-Length` header are not limited at the consumer level.

**Response Bodies:** 
- `Body::Stream` responses use `Transfer-Encoding: chunked` automatically (no buffering)
- `Body::Bytes` / `Body::Text` responses use standard `Content-Length`

### Streaming Response Example

```rust
// Streaming response example (server-sent data)
from("http://0.0.0.0:8080/stream")
    .process(|exchange| Box::pin(async move {
        let chunks = vec![
            Ok(Bytes::from("chunk1\n")),
            Ok(Bytes::from("chunk2\n")),
        ];
        let stream = Box::pin(futures::stream::iter(chunks));
        exchange.input.body = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(stream))),
            metadata: StreamMetadata::default(),
        });
        Ok(())
    }))
    .build()
```

### Request Body Access

```rust
// Access request body as stream (default) or materialize it
from("http://0.0.0.0:8080/upload")
    .process(|exchange| Box::pin(async move {
        // Option A: keep as stream (zero-copy)
        // exchange.input.body is already Body::Stream
        
        // Option B: materialize when you need the bytes
        let bytes = exchange.input.body.into_bytes(10 * 1024 * 1024).await?;
        exchange.input.body = Body::Bytes(bytes);
        Ok(())
    }))
    .build()
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-http)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
