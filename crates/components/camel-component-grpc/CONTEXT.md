# camel-component-grpc

Dynamic gRPC component for rust-camel — provides bidirectional gRPC producer and consumer with runtime proto resolution via `prost-reflect`. No compile-time code generation required: proto files are resolved at runtime through `camel-proto-compiler`. Supports unary, server-streaming, client-streaming, and bidirectional streaming — mode is auto-detected from the proto method descriptor.

## Language

**GrpcEndpoint**:
Endpoint for `grpc://host:port/package.Service/Method?protoFile=…` URIs; resolves proto descriptors at runtime via `camel-proto-compiler`.
_Avoid_: grpc address, grpc stub

**GrpcConsumer**:
gRPC server-side Consumer; binds one HTTP/2 listener per `(host, port)`, dispatches by path to the matching route. Mode (unary, server-streaming, client-streaming, bidi) is auto-detected from the proto method descriptor.
_Avoid_: grpc server, grpc handler

**GrpcProducer**:
gRPC client Producer; sends Exchanges as gRPC calls. Holds an internal lazy pool of connections and reports endpoint health via `RuntimeObservability`. **Batch 1 (C1):** when `tls = true`, the producer hard-errors at construction unless a `TlsConfig` is supplied — no silent fallback to h2c plaintext, so auth tokens never travel cleartext. `tls = false` with a `TlsConfig` supplied also hard-errors (conflicting intent). The `ClientTlsConfig` is wired from `TlsConfig` (server_name, optional ca_cert, optional mTLS identity); the channel's endpoint URL is rewritten to `https://`. `parse_grpc_uri` rejects `?tls=true` at parse time (a URI cannot carry certificates; TLS-from-URI cert params are a filed follow-up). `insecure_skip_verify=true` hard-errors (verifier shim needs a follow-up batch); the default `false` is safe.
_Avoid_: grpc client, grpc caller

**GrpcStreamObserver**:
Stream observer API (`on_next` / `on_error` / `on_completed`) used by Routes to send streaming responses.
_Avoid_: stream writer, response stream

**Shared HTTP/2 server**:
One HTTP/2 server per `(host, port)`; multiple GrpcConsumers register dispatch by URI path.
_Avoid_: server pool, connection multiplex

**Route-stable ID**:
For cross-route infra (e.g., `e:grpc:accept` accept-loop failures), the route_id is the stable string `format!("grpc-server:{addr}")` (Q-B1 oracle). Per-route sites use the owning route's actual route_id.
_Avoid_: server route id, listener id

## Log-level policy

Per ADR-0012.

**Labels wired in Phase B (commit 8455aa70 + rework 269094f8):**
- `e:grpc:accept` (`src/server.rs:193` accept-loop failure) — category (e) cross-route infra: HTTP/2 listener cannot accept new connections. Uses stable route_id `"grpc-server:{addr}"` (Q-B1 oracle) since the failure spans all routes sharing the listener. Calls `runtime.metrics().increment_errors(route_id, label)` then logs at `error!` with `// log-policy: outside-contract`.

**Labels wired in Phase B (commit 089cffd8):**
All 5 sites are category (g) endpoint-creation failures — the producer's creation cannot establish a gRPC channel to the target. Each site calls `runtime.health().force_unhealthy_for_route(route_id, label, reason)` then logs at `error!` with `// log-policy: outside-contract`.
- `g:grpc:producer-create` (`src/producer/mod.rs:95` channel open failure)
- `g:grpc:producer-create` (`src/producer/mod.rs:110` proto compilation failure)
- `g:grpc:producer-create` (`src/producer/mod.rs:124` service descriptor not found)
- `g:grpc:producer-create` (`src/producer/mod.rs:141` method descriptor not found)
- `g:grpc:producer-create` (`src/producer/mod.rs:154` invalid gRPC path)

All five sites use the same label `"g:grpc:producer-create"` (Q-B2 oracle — no helper extraction, sites inlined).

## Example dialogue

> "How do I configure a gRPC consumer for unary vs streaming?"
> "You don't need to — the mode is auto-detected from the proto method descriptor. The same GrpcConsumer handles unary, server-streaming, client-streaming, and bidi based on the proto definition."

> "What happens when the gRPC server's accept loop fails?"
> "It calls `increment_errors` with label `e:grpc:accept` using the stable route_id `grpc-server:{addr}` (since the failure affects all routes sharing that listener), logs at `error!` with `// log-policy: outside-contract`, and continues accepting."
