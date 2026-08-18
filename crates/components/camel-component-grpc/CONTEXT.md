# camel-component-grpc

Dynamic gRPC component for rust-camel — provides bidirectional gRPC producer and consumer with runtime proto resolution via `prost-reflect`. No compile-time code generation required: proto files are resolved at runtime through `camel-proto-compiler`. Supports unary, server-streaming, client-streaming, and bidirectional streaming — mode is auto-detected from the proto method descriptor.

## Language

**GrpcEndpoint**:
Endpoint for `grpc://host:port/package.Service/Method?protoFile=…` URIs; resolves proto descriptors at runtime via `camel-proto-compiler`.
_Avoid_: grpc address, grpc stub

**GrpcConsumer**:
gRPC server-side Consumer; binds one HTTP/2 listener per `(host, port)`, dispatches by path to the matching route. Mode (unary, server-streaming, client-streaming, bidi) is auto-detected from the proto method descriptor. Inbound TLS termination when `transport=tls` + `serverCertPath`/`serverKeyPath` (server-auth only). mTLS client-cert verification when `clientCaPath` is also set (fail-closed: clients without a valid cert are rejected). The shared-server registry refuses to mix TLS/plaintext on one listener (transport-mismatch hard-error).
_Avoid_: grpc server, grpc handler

**GrpcProducer**:
gRPC client Producer; sends Exchanges as gRPC calls. Holds an internal lazy pool of connections and reports endpoint health via `RuntimeObservability`. Transport intent is `ClientTransport::Plaintext` or `ClientTransport::Tls(ClientTlsConfig)` — `transport=plaintext|tls` is REQUIRED in the URI (ADR-0033). The legacy `tls=true/false` key is rejected. Under `Tls`, the producer wires `ClientTlsConfig` (server_name, optional ca_cert, optional mTLS identity via `clientCertPath`/`clientKeyPath`); the channel's endpoint URL is rewritten to `https://`. `insecure_skip_verify=true` hard-errors (fail-closed). Incomplete mTLS identity (cert without key or vice versa) is rejected.
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

## Trust boundary and authorization enforcement

ADR-0032 classifies inbound gRPC metadata and protobuf payloads as untrusted.
`extract_metadata` copies metadata into Exchange headers and skips reserved
transport keys. These keys include `content-type`, `te`, `grpc-*`, and
`user-agent`. `DynamicMessage::decode` decodes the payload without a panic path.

Security enforcement has two layers:

1. The server handler authenticates the request. `extract_principal` maps the
   inbound metadata to an HTTP header view and extracts the token through the
   shared `camel_auth::extract_token_multi` using the route-declared
   `credential_sources` (default `[authorization_header]`, so the historical
   `authorization: Bearer <token>` behavior is unchanged). It then calls
   `TokenAuthenticator::authenticate_bearer`. Missing or invalid credentials
   return `Status::unauthenticated`. An unavailable provider returns
   `Status::unavailable`. An unexpected authenticator failure returns
   `Status::internal` and emits the category (h) log. `query_param` and
   `cookie` sources are rejected at route load (`validate_credential_sources`)
   because gRPC metadata cannot carry them.
2. `GrpcConsumer` authorizes before pipeline dispatch, as required by
   ADR-0010. It calls `SecurityPolicy::evaluate` in unary, server-streaming,
   client-streaming, and bidirectional modes. `Denied` returns
   `Status::permission_denied`. Evaluation errors return `Status::internal`.
   Unknown future decisions fail closed as `Status::permission_denied`.

Transport setup also fails closed under ADR-0033. Every Endpoint declares
`transport=plaintext|tls`. The component rejects `insecure_skip_verify=true`,
an incomplete mTLS identity, and a TLS/plaintext mismatch on a shared listener.

## Log-level policy

Per ADR-0012. This is the complete non-test inventory of 29 `error!` and
`warn!` sites. Symbol names are authoritative because line numbers change.

### Category (e): cross-route infrastructure

- `run_grpc_server` has one accept-loop `error!`. It increments
  `e:grpc:accept` for the stable route ID `grpc-server:{addr}` before logging.

### Category (g): Endpoint and Producer creation

- `GrpcProducer::new` has six `error!` sites: invalid Endpoint, rejected
  `insecure_skip_verify`, proto compilation, missing service descriptor,
  missing method descriptor, and invalid gRPC path. Each site uses
  `g:grpc:producer-create` and forces the owning route unhealthy.
- `GrpcServerRegistry::get_or_spawn` and
  `GrpcServerRegistry::get_or_spawn_with_listener` each have one TLS-config
  build `error!`. Both use `g:grpc:tls-read` and force the shared server route
  unhealthy.
- `read_tls_file` has one TLS-file `error!`. It uses `g:grpc:tls-read` and
  forces the owning route unhealthy.

### Category (h): pre-pipeline security faults

- `GrpcConsumer::start_inner` has four policy-evaluation `error!` sites, one
  for each RPC mode.
- `extract_principal` has one authentication `error!` for an unexpected
  authenticator failure.

### Handler-owned warnings

- `GrpcConsumer::start_inner` has four authorization-denied `warn!` sites, one
  for each RPC mode.
- The client-stream and bidirectional-stream handlers each have one decode
  `warn!`.
- The producer retry loop has one non-retryable-status `warn!`.
- Producer protobuf conversion has four `warn!` sites.

### Operational warnings and no-ops

- `GrpcReloadHandler::reload` has one `warn!` when reload fails and the old
  certificate remains active.
- URI parsing has two `warn!` sites: an unrecognized parameter and the ignored
  `reflection=true` option.

## Example dialogue

> "How do I configure a gRPC consumer for unary vs streaming?"
> "You don't need to — the mode is auto-detected from the proto method descriptor. The same GrpcConsumer handles unary, server-streaming, client-streaming, and bidi based on the proto definition."

> "What happens when the gRPC server's accept loop fails?"
> "It calls `increment_errors` with label `e:grpc:accept` using the stable route_id `grpc-server:{addr}` (since the failure affects all routes sharing that listener), logs at `error!` with `// log-policy: outside-contract`, then sleeps with capped exponential backoff (`BackoffState`, 10ms to 5s) before continuing. Resets on successful accept."
