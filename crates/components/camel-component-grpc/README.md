# camel-component-grpc

> gRPC component for rust-camel (dynamic producer)

## Overview

`camel-component-grpc` provides a dynamic gRPC producer for rust-camel, using runtime proto resolution to call gRPC methods without compile-time code generation.

## Features

- Dynamic gRPC producer (no codegen required at compile time)
- JSON in → gRPC call → JSON out
- Proto files resolved at runtime via `camel-proto-compiler`
- Raw bytes codec for transparent serialization
- Header forwarding from Exchange to gRPC metadata
- Feature-gated behind `grpc` feature in `camel-cli`

## Installation

Enable `grpc` in `camel-cli`:

```toml
# Cargo.toml for camel-cli
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

```rust
// In Camel.toml
[default.components.grpc]

// In route
let route = RouteBuilder::from("timer:tick?period=5000")
    .set_body(Body::Json(json!({"name": "World"})))
    .to("grpc://localhost:50051/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto")
    .to("log:grpc-response")
    .build()?;
```

## License

Apache-2.0
