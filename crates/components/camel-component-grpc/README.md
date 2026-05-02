# camel-component-grpc

> gRPC component for rust-camel (dynamic producer and consumer with streaming)

## Overview

`camel-component-grpc` provides dynamic gRPC producer and consumer for rust-camel, using runtime proto resolution via `prost-reflect`. No compile-time code generation required — proto files are resolved at runtime through `camel-proto-compiler`. Supports unary, server streaming, client streaming, and bidirectional streaming — mode is auto-detected from the proto method descriptor.

## Features

- **gRPC Server (Consumer)**: Listen for incoming gRPC requests (unary, server streaming, client streaming, bidi)
- **gRPC Client (Producer)**: Make outgoing gRPC calls (unary, server streaming, client streaming, bidi)
- **Dynamic proto resolution**: No codegen required at compile time
- **JSON pipeline**: Automatic protobuf ↔ JSON conversion
- **StreamObserver API**: Routes send streaming responses via `GrpcStreamObserver` (on_next/on_error/on_completed)
- **Metadata propagation**: gRPC metadata → Exchange headers (including binary `-bin` as base64)
- **Shared server**: One HTTP/2 server per `(host, port)`, multiple consumers dispatch by path
- **Feature-gated** behind `grpc` feature in `camel-cli`

## Installation

Enable `grpc` in `camel-cli`:

```toml
camel-cli = { version = "*", features = ["grpc"] }
```

## URI Format

```
grpc://host:port/package.Service/Method?protoFile=path.proto
```

## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `protoFile` | required | Path to `.proto` file |
| `reflection` | false | (v2) gRPC reflection |
| `tls` | false | (v2) TLS support |
| `deadline_ms` | none | Request deadline |
| `metadata` | none | Static metadata headers |

## Usage

### gRPC Server (Consumer)

```rust
use camel_builder::RouteBuilder;
use camel_component_grpc::GrpcComponent;
use camel_core::CamelContext;

let mut ctx = CamelContext::new();
ctx.register_component(GrpcComponent::new());

// Listen for gRPC requests, echo response
let route = RouteBuilder::from("grpc://0.0.0.0:50051/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto")
    .set_body(Body::Json(json!({"message": "Hello World"})))
    .build()?;
```

The consumer:
- Starts an HTTP/2 server on the specified host:port
- Dispatches requests by gRPC path (`/package.Service/Method`)
- Decodes protobuf → JSON, sends through the pipeline
- Encodes the pipeline response JSON → protobuf

### gRPC Client (Producer)

```rust
// Timer-triggered gRPC calls
let route = RouteBuilder::from("timer:tick?period=5000")
    .set_body(Body::Json(json!({"name": "World"})))
    .to("grpc://localhost:50051/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto")
    .to("log:grpc-response")
    .build()?;
```

### Shared Server

Multiple consumers on the same `(host, port)` share one HTTP/2 server. Each consumer registers a unique gRPC path:

```rust
let route1 = RouteBuilder::from("grpc://0.0.0.0:50051/pkg.SvcA/Method1?protoFile=service.proto")
    .set_body(Body::Json(json!({"result": "A"})))
    .build()?;

let route2 = RouteBuilder::from("grpc://0.0.0.0:50051/pkg.SvcB/Method2?protoFile=service.proto")
    .set_body(Body::Json(json!({"result": "B"})))
    .build()?;
```

## Exchange Headers

### gRPC Metadata → Exchange Headers (Consumer)

gRPC metadata is forwarded to exchange headers. Binary metadata (keys ending in `-bin`) is base64-encoded with a `bin:` prefix.

## Streaming

Streaming mode is auto-detected from the proto method descriptor. No configuration needed — if your proto defines a streaming method, the component handles it.

### Consumer Streaming (Server Streaming)

Routes access `GrpcStreamObserver` to send streaming responses:

```rust
use camel_component_grpc::consumer::take_stream_observer;

// In your route processor:
if let Some(observer) = take_stream_observer(&exchange) {
    observer.on_next(json!({"index": 0, "name": "item-0"})).await?;
    observer.on_next(json!({"index": 1, "name": "item-1"})).await?;
    observer.on_completed().await;
}
```

### Producer Streaming

- **Server streaming**: Send one request, receive JSON array of responses
- **Client streaming**: Send JSON array of requests, receive one response
- **Bidi streaming**: Send JSON array of requests, receive JSON array of responses

## License

Apache-2.0
