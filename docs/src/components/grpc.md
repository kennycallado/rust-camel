# gRPC

The gRPC component produces and consumes gRPC with runtime proto resolution. No compile-time code generation is required. The component resolves `.proto` files at runtime through `camel-proto-compiler` and `prost-reflect`. It supports unary, server-streaming, client-streaming, and bidirectional streaming. The mode is auto-detected from the proto method descriptor.

The grpc-example wires a consumer on port 50051 and a timer-driven producer:

```rust,ignore
{{#include ../../../examples/grpc-example/src/main.rs:grpc-consumer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: grpc-consumer
    from: "grpc://0.0.0.0:50051/helloworld.Greeter/SayHello?protoFile=helloworld.proto&transport=plaintext"
    steps:
      - set_body:
          value:
            message: Hello from consumer!
      - to: "log:grpc-consumer?showBody=true"
```

The Rust example builds `protoFile` from `CARGO_MANIFEST_DIR`. Substitute the real path to your `.proto` file. The `transport=plaintext` parameter is required (ADR-0033).

</details>

```rust,ignore
{{#include ../../../examples/grpc-example/src/main.rs:grpc-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: grpc-producer
    from: "timer:grpc-tick?period=3000&repeatCount=3"
    steps:
      - set_body:
          value:
            name: World
      - to: "grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=helloworld.proto&transport=plaintext"
      - to: "log:grpc-response?showBody=true"
```

The Rust example builds `protoFile` from `CARGO_MANIFEST_DIR`. Substitute the real path to your `.proto` file.

</details>

## URI

```text
grpc://<host>:<port>/<package>.<Service>/<Method>?protoFile=<path>&transport=<mode>
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `protoFile` | yes | — | Path to the `.proto` file for runtime descriptor resolution |
| `transport` | yes | — | `plaintext` or `tls` (ADR-0033) |
| `serverCertPath` | consumer (tls) | — | Path to the server TLS certificate |
| `serverKeyPath` | consumer (tls) | — | Path to the server TLS key |
| `clientCaPath` | consumer (mtls) | — | Path to the client CA certificate for mTLS |
| `clientCertPath` | producer (mtls) | — | Path to the client TLS certificate |
| `clientKeyPath` | producer (mtls) | — | Path to the client TLS key |

## Consumer

`grpc://0.0.0.0:50051/helloworld.Greeter/SayHello?protoFile=helloworld.proto&transport=plaintext` binds an HTTP/2 listener. The Consumer dispatches each inbound gRPC request to the Route. The Exchange body carries the decoded protobuf message as JSON. The Exchange headers carry gRPC metadata.

Multiple GrpcConsumers on the same `(host, port)` share one HTTP/2 server. Each consumer registers dispatch by URI path. The shared-server registry refuses to mix TLS and plaintext on one listener.

The Consumer supports four RPC modes. It auto-detects the mode from the proto method descriptor. The same Consumer handles unary, server-streaming, client-streaming, and bidirectional calls without configuration changes.

## Producer

`grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=helloworld.proto&transport=plaintext` sends the Exchange body as a gRPC call. The Producer holds a lazy pool of connections. It reports endpoint health through `RuntimeObservability`.

The Producer requires `transport=plaintext` or `transport=tls` in the URI (ADR-0033). The legacy `tls=true` and `tls=false` keys are rejected. Under `tls`, the endpoint URL is rewritten to `https://`. The `insecure_skip_verify=true` option hard-errors. The component fails closed on an incomplete mTLS identity.

## Security

The component enforces two security layers (ADR-0010, ADR-0032):

1. **Authentication**. The server handler parses `Authorization: Bearer <token>` and calls `TokenAuthenticator::authenticate_bearer`. Missing or invalid credentials return `Status::unauthenticated`.
2. **Authorization**. The Consumer calls `SecurityPolicy::evaluate` before pipeline dispatch in all four RPC modes. `Denied` returns `Status::permission_denied`. Unknown future decisions fail closed.

Transport setup also fails closed (ADR-0033). Every Endpoint declares `transport=plaintext` or `transport=tls`. The component rejects `insecure_skip_verify=true`, an incomplete mTLS identity, and a TLS/plaintext mismatch on a shared listener.

## Streaming

Routes that send streaming responses use `GrpcStreamObserver`. The observer exposes three methods: `on_next`, `on_error`, and `on_completed`. The route calls these methods to push response messages onto the gRPC stream.

**Reference**: [gRPC crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-grpc/CONTEXT.md). Example source: [`examples/grpc-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/grpc-example).
