# WebSocket component

This file defines crate-local terms and invariants. See `../CONTEXT.md` for the shared
Component, Endpoint, Consumer, and Producer terms.

## Language

**`WsComponent` / `WssComponent`**:
Components for plain and TLS WebSocket Endpoints. Each Endpoint can create an inbound Consumer or
an outbound Producer.

**`ServerRegistry`**:
Process-wide owner of shared inbound servers. It keys servers by port and keeps them alive after
the last Consumer stops. Paths can register and deregister independently on one server.

**`WsConnectionRegistry`**:
Per-path registry of active WebSocket connections. Producers use it for targeted or broadcast
delivery.

 **`dispatch_handler`**:
Inbound upgrade handler. It checks path and origin, then runs the unified transport auth kernel
handshake (`WsKernelAuth`) when the route carries a compiled security plan and providers; without a
plan it passes through as Public with no credential extraction. Authorization runs in the pipeline
security layer against the kernel carrier.

## Client-consumer role

**`WsClientConsumer`**:
Consumer built when a route's `from:` endpoint carries `consumeAsClient=true`. It dials out to the
endpoint's host/port/path as a WebSocket client and turns each pushed frame into one Exchange
(`Text` → String body, `Binary` → bytes body, header `CamelWsMessageType` = `text`/`binary`;
`Ping`/`Pong` are transparent). `consumeAsClient` is a strict boolean URI parameter, default
`false`; absent or `false` keeps the existing server mode (`WsConsumer`), so the option is an
applicability switch, not a third role. `heartbeatIntervalMs`/`idleTimeoutMs` do not apply in
client mode: protocol-level pings are answered by the continuous read loop, and client-side
heartbeat emission was deliberately excluded. Frames larger than `maxMessageSize` are dropped with a
`warn!` and an error metric; the connection and subsequent frames keep flowing. Concurrency is
`Sequential`; delivery goes through the bounded consumer-to-pipeline channel, so reads pause while
the route is backpressured. Connect observability is owned solely by the retry helper
(ADR-0066 D6/D13); frame outcomes flow through the lever-gated component-metrics facade
(`observe("ws", "frame", failed)`).

**`ClientConnState`**:
Connection lifecycle published on a `watch` channel: `Connecting` (attempt in flight),
`Connected` (handshake done, frames flow), `Reconnecting` (transient failure, bounded reconnect
sequence in flight), `Exhausted` (reconnect policy exhausted). Each disconnect starts ONE fresh
bounded sequence via `NetworkRetryPolicy` (`reconnect`/`reconnectMaxAttempts`/`reconnectDelayMs`
URI bridging); exhaustion fails the consumer task so Route supervision sees it. Startup is
`ConsumerStartupMode::Explicit`: `mark_ready` only after the first successful connect, fail-loud
`Err` when the initial connect exhausts the policy. The passive `ws-client` health check reads the
watch (no probe connections); `Connected` is healthy, every other state is unhealthy.

**TLS posture**:
Production `wss://` client connects use native root certificates (tokio-tungstenite
`rustls-tls-native-roots` feature; connector `None`). The feature is crate-wide, so the unchanged
producer's `connect_async` also becomes TLS-capable — an acknowledged side effect: `wss://`
producer routes that previously failed on connect now work. Tests inject a test-CA connector for
deterministic trust.

## Lifecycle and security invariants

- `ServerRegistry::get_or_spawn` creates at most one server per port. The first registration fixes
  the host and TLS mode. A later registration with the other TLS mode fails.
- `ServerRegistry::release` removes the Consumer reference but does not stop the server. Consumer
  shutdown removes its path, policy, and connection registry.
- `WsReloadHandler::matches` matches `wss` servers by port and intentionally ignores host. This
  mirrors the port-keyed `ServerRegistry`.
- Plain WS binds its TCP listener before `ConsumerContext::mark_ready`. WSS also binds its
  listener before delegation (`spawn_server` serves a pre-bound listener via `from_tcp_rustls`);
  `listening()` now signals that the serve/accept loop actually started. `WsConsumer::start`
  awaits `axum_server::Handle::listening()` before signalling readiness. On serve failure
  (`listening()` returns `None`) `start` returns `Err`, so the route
  never marks itself ready on a dead listener. The health pin
  (`g:ws:bind-tls`) and the `WsConsumer::stop` error remain secondary failure
  signals for bind errors that surface after readiness. ADR-0007 requires
  such task failures to remain visible to Route supervision.
- Test/operator surface (bd rc-9xsv, port-toctou elimination):
  `ServerRegistry::get_or_spawn_with_listener` accepts a pre-bound
  `tokio::net::TcpListener`, keys the registry by the listener's actual port,
  and returns the entry's stored `bound_addr` (`ServerHandle` payload).
  `WsConsumer::start_with_listener` mirrors it on the consumer side — the
  URI's host:port is informational; the listener is authoritative for
  binding (connection-registry keys use the bound address, see the
  in-code note on the `canonical_host` asymmetry).
- Staged listeners (bd rc-h0aw): `ServerRegistry::stage_listener` parks a
  pre-bound listener under its exact `(host, port)` key, one-shot.
  `get_or_spawn` consumes it only when creating a vacant entry (single
  resolver inside the init winner); an existing entry never touches the
  staged slot. A staged listener on the same port under a different host
  string fails the spawn deterministically (`staged listener conflict on
  port …`) instead of risking `EADDRINUSE`. Unclaimed staged listeners
  are dropped at process end (test bug, not runtime hazard).
- When a security plan exists, the kernel handshake fails closed: missing or invalid credentials
  reject the upgrade (401) and install no carrier, and the strict dispatch check denies a
  non-Public route whose exchange lacks the carrier. Query-token values are redacted before
  logging.
- A route may declare `credential_sources` (ADR-0059). The kernel resolves them in declared order
  through the shared camel-auth extraction and authenticates the token; the resulting principal
  rides the kernel carrier and the pipeline policy layer evaluates it. The removed
  `trust_upstream_principal` flag no longer exists and exchange-property principal evidence never
  authorizes. URI logging redacts every declared query-parameter source name, plus the
  `access_token` and `token` defaults.
- `WsEndpointConfig::fmt` redacts TLS certificate and key paths. ADR-0051 does not classify paths
  as credential bytes, so this crate uses a stricter diagnostic policy.
- ADR-0052 does not apply. `camel-ws` is an inbound data-plane component, not a diagnostic
  endpoint. Its `0.0.0.0` default therefore does not inherit the diagnostic loopback rule.

## Log-level policy

Per ADR-0012, this component's `error!` sites are outside the handler contract:

- **Class (g)** (`spawn_server`, TLS-bind arm): the code first calls
  `force_unhealthy_for_route` with `g:ws:bind-tls`. The health pin is the operator signal.
- **Class (g)** (`spawn_server`, plain-bind arm): the code first calls
  `force_unhealthy_for_route` with `g:ws:bind-plain`. The health pin is the operator signal.

Kernel authentication rejections in `dispatch_handler` log at `warn!` and increment
`e:ws:authn` first; the metric is the operator signal.

Client-consumer oversized-frame drops log at `warn!` and increment
`increment_errors(route_id, "ws_client_consumer")` first; the metric is the operator signal.
Dispatch failure (route channel closed) increments the same metric and the task returns
`Err` — no log, the error return is the signal.

Each `error!` site keeps the level for loud log visibility and carries `// log-policy: outside-contract`.
