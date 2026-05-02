# camel-component-grpc

> gRPC component for rust-camel (dynamic producer and consumer)

## Overview

`camel-component-grpc` provides dynamic gRPC producer and consumer for rust-camel, using runtime proto resolution via `prost-reflect`. No compile-time code generation required — proto files are resolved at runtime through `camel-proto-compiler`.

## Features

- **gRPC Server (Consumer)**: Listen for incoming unary gRPC requests
- **gRPC Client (Producer)**: Make outgoing unary gRPC calls
- **Dynamic proto resolution**: No codegen required at compile time
- **JSON pipeline**: Automatic protobuf ↔ JSON conversion
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

## License

Apache-2.0
