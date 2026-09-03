# Proposal: ws-client-consumer

## Why

Nodes migrating from Java Camel use vertx-websocket with `consumeAsClient=true`:
the route's `from:` endpoint dials OUT as a WebSocket client to a third-party
server, and every server-pushed frame becomes a new exchange (consumer
pattern, not request/response). rust-camel 0.38.0 cannot express this:

- `WsEndpoint::create_consumer` always builds `WsConsumer` in server mode
  (accepts inbound connections, cannot dial out).
- `WsProducer` dials out but is purely request/response: connect, send one
  message, await one response. No continuous receive loop reinjecting frames.

This is the only migration blocker for the node in question (HTTP
consumer/producer, XSLT via camel-xj, TLS-insecure already have direct
equivalents). Without it, users must fall back to a custom WASM plugin or an
external sidecar republishing into `direct:`/`seda:` — both avoidable.

## What Changes

- camel-ws gains a `consumeAsClient` URI option (boolean, default `false`).
  Parity scope is deliberate and narrow: the option SPELLING matches Java
  vertx-websocket, and the BEHAVIOR parity is "outbound client connection
  whose pushed frames become exchanges". This is NOT full vertx-websocket
  component parity — client authentication options, client-side heartbeat
  emission, and frame-masking knobs are out of scope (excluded below).
- New `WsClientConsumer`: a Consumer created from a route's `from:` endpoint
  when `consumeAsClient=true`. It establishes the outbound connection using
  the existing retry machinery (`retry_async_cancelable` +
  `NetworkRetryPolicy`), then runs a continuous, cancellation-aware receive
  loop that turns each text/binary frame into a new Exchange submitted to the
  route pipeline via `ConsumerContext::sender()`.
- Reconnect with backoff on disconnect (each disconnect starts one fresh
  bounded reconnect sequence; exhaustion surfaces as consumer failure for
  route supervision). Startup readiness follows `ConsumerStartupMode::Explicit`
  (`mark_ready()` on first successful connect).
- `wss://` client connections work through a dedicated client-consumer TLS
  path: the `rustls-tls-native-roots` feature is enabled on
  tokio-tungstenite and the consumer connects via
  `connect_async_tls_with_config` (native roots in production, test-CA root
  store in tests). The feature is crate-wide, so the unchanged producer
  `connect_async` also becomes TLS-capable — an acknowledged, tested side
  improvement: `wss://` producer routes that previously failed on connect
  now work (see risk budget).
- Passive, connection-state health check (Connecting/Connected/Reconnecting/
  Exhausted) registered only in client mode; the existing TCP-listener check
  is untouched for server mode.
- Metrics per ADR-0066 ownership: connect observability owned solely by
  `retry_async_cancelable` (per-attempt counters + single exhaustion error);
  frame outcomes through the lever-gated component-metrics facade
  (`component_metrics().observe("ws", "frame", failed)`).
- Docs updated: camel-ws CONTEXT.md, CONTEXT-MAP.md key term, README.

Affected crates: `crates/components/camel-ws` (primary).
`camel-component-api` unchanged (all needed contracts exist). Lint catalog
picks up the new option automatically via the `#[uri_param]` metadata macro.

Excluded: producer request/response behavior, server-mode behavior, new
dependencies, remote-parent otel context propagation from inbound frames,
handshake header injection (no ambient exchange context exists at consumer
`start()`), client authentication/heartbeat/masking options.

Bd: rc-39d6.

## Acceptance criteria

- `from("ws://...?consumeAsClient=true")` connects outbound; each pushed frame
  arrives in the route as a new exchange with correct body type.
- Disconnect triggers automatic reconnect per policy; frames after reconnect
  continue flowing without route restart.
- Remote unreachable at startup: route start fails loud once the reconnect
  policy is exhausted (no silent no-op route).
- Default server mode is bit-identical when the option is absent; existing
  camel-ws test suite passes unchanged.
- `camel lint` accepts the new URI option (metadata-derived, no manual
  catalog edit).

## Risk budget

- Server-mode regression is the main risk; contained by keeping the new role
  behind the new option and leaving `WsConsumer` untouched.
- Metadata surface grows by one option; drift breaks lint — covered by
  metadata tests.
- No security-relevant surface changes: outbound client connection, no new
  inbound trust decision.
- Crate-wide TLS feature: `wss://` producer routes that previously failed on
  connect start working — strictly an improvement, covered by a dedicated
  producer wss test; `ws://` paths are feature-neutral.
