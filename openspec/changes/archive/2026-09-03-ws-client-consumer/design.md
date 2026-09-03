# Design: ws-client-consumer

## Approach

Add a third behavior to camel-ws: a consumer that dials out. Separate type,
`WsClientConsumer` — the server consumer carries registry, app-state,
TLS-reload and path-policy machinery irrelevant to a single outbound
connection.

**Configuration.** One new `#[uri_param]` on `WsUriConfig`:
`consumeAsClient` (bool, default `false`), stored on `WsEndpointConfig`,
parsed in `from_uri`, exposed via the macro in `ComponentMetadata` and
`uri_options()` (no manual catalog edit). Option applicability matrix:

| Option | Server consumer | Client consumer | Producer |
|---|---|---|---|
| consumeAsClient | selects role | selects role | n/a |
| maxConnections | yes | n/a (single conn) | n/a |
| maxMessageSize | yes | **enforced inbound** | n/a |
| heartbeatIntervalMs / idleTimeoutMs | yes | n/a (protocol-level ping answered by continuous reads) | n/a |
| allowOrigin, tlsCert, tlsKey | yes | n/a | n/a |
| connectTimeoutMs, reconnect*, subprotocols | connect n/a | yes | yes |
| binaryPayload, responseTimeoutMs, sendTimeoutMs, sendToAll | n/a | n/a | yes |

`binaryPayload` is producer-only today (it selects the outbound producer
frame type, WS-018); the client-consumer detects frame type from the wire
variant (`Text`/`Binary`) and needs no option for mapping.

**Wiring.** `WsEndpoint::create_consumer` branches on the flag:
`consumeAsClient=true` builds `WsClientConsumer::new(cfg.client_config(), rt,
conn_state_tx)` (third arg: the connection-state watch sender threaded from
`create_endpoint` — see Health), else the existing
`WsConsumer::new(cfg.server_config(), rt)`. `create_producer`
untouched. `WssComponent::create_endpoint` is a textual duplicate of
`WsComponent::create_endpoint` (not a delegation) — both bodies are edited
identically, so both schemes
inherit the behavior.

**Connection.** Build the client request exactly as the producer does (URL
from scheme/host/port/path; `Sec-WebSocket-Protocol` header when subprotocols
configured). Connect through a NEW free function
`connect_ws_client_cancelable` wrapping the existing
`retry_async_cancelable(&policy, "ws", "connect", op, is_retryable_ws_error,
&cancel, metrics_opt)` — public signature takes `cancel: &CancellationToken`
(plain ref) and `metrics: Option<&dyn MetricsCollector>`. Metrics access:
retain the `Arc<dyn MetricsCollector>` returned by
`RuntimeObservability::metrics()` and pass `Some(metrics.as_ref())`. Per
ADR-0066 D6/D13 the retry helper is then the SOLE owner of connect
observability: attempts via `increment_retry_attempt("ws","connect")` and
exhaustion via its single `increment_errors("connect","e:ws:connect")` —
the call site adds NO additional connect metrics (no double ownership, no
`record_component_operation` on `connect`; a distinct operation label would
be required if ever added). The helper call is wrapped in a biased
`tokio::select!` against `cancel.cancelled()` so cancellation during connect
or backoff yields a clean, explicit outcome instead of an indistinguishable
network `Err`; only the helper's genuine `Err` arm may publish the
`Exhausted` state. The producer's `connect_ws_with_retry` (uncancelable,
`metrics=None`) is left untouched. Initial connect: exhaustion returns `Err`
from `start()` (fail-loud unreachable remote). On success: `ctx.mark_ready()`,
spawn the task.

**TLS (wss).** The workspace pins `tokio-tungstenite = "0.30"` with
default features only — its `connect_async` path has NO TLS support today.
This change adds the `rustls-tls-native-roots` feature CRATE-SCOPED:
camel-ws declares `tokio-tungstenite = { workspace = true, features =
["rustls-tls-native-roots"] }` (no new dependency, feature addition only;
other workspace consumers such as camel-test stay on default features).
The client-consumer connects via
`connect_async_tls_with_config`: production passes connector `None` (with
the feature enabled, tokio-tungstenite loads native roots itself); tests
pass `Some(Connector::Rustls(test_config))` whose root store contains only
the test CA (deterministic trust, no system-state dependence). The feature
is crate-wide: the UNCHANGED producer `connect_async` (which delegates to
the same TLS-capable path with connector None) also becomes TLS-capable —
`wss://` producer routes that previously failed on connect now work. This
side improvement is acknowledged and covered by a producer wss test reusing
the same test-CA TLS server fixture; no producer code changes.

**State machine.** Consumer field `state: Created | Running | Stopped`.
`start()` on `Running` returns `Err` (double-start rejected, same stance as
server WS-006). Task ownership follows the `Consumer` shutdown contract:
`background_task_handle()` is called at most once and `.take()`s the primary
JoinHandle into Runtime supervision, so `stop()` MUST NOT depend on taking
that handle back. `stop()` cancels the stored `ConsumerContext` token (the
task already selects on it), performs idempotent state cleanup, and awaits a
handle only when one is still locally owned (direct-use/test paths where
`background_task_handle()` was never called). The Runtime cancels the token
and calls `stop()` on every task exit path. Task loop each iteration:
`tokio::select!` over `stream.next()` and `cancel.cancelled()`. On cancel:
best-effort `ws_stream.close(...)` (Close frame), exit `Ok`. On disconnect /
stream error / Close frame: start ONE fresh bounded reconnect sequence via
`connect_ws_client_cancelable`; exhaustion exits the task with `Err` (route
supervision sees consumer failure; no silent infinite retry past the bound).
Cancellation is honored mid-read, mid-backpressure-send, and mid-retry-sleep.

**Frame handling.** `Text` → String body, `Binary` → bytes body,
`Ping`/`Pong` → transparent (tokio-tungstenite answers pings while the stream
is read; continuous reads keep the connection alive). Each frame becomes an
`Exchange` (header `CamelWsMessageType` = `text`/`binary`) in an
`ExchangeEnvelope` with `reply_tx: None`, sent via `ctx.sender()` — the
bounded mpsc send awaits under route backpressure (loop pauses reads; no
unbounded buffer). Frame payload exceeding `maxMessageSize` is dropped with a
`warn!` log and an error metric; the connection stays up and subsequent
frames flow (per-message limit, not connection teardown).

**Health.** The existing `WsHealthCheck` is a TCP-listener probe — wrong
instrument for client mode. Health registration happens at the API boundary
that actually offers it: `ConsumerContext` has NO health-registration
method; `ComponentContext::register_current_route_health_check` does (already
used in `create_endpoint`). Wiring: `WsComponent`/`WssComponent::
create_endpoint` creates the shared `watch::channel(ClientConnState)`
(`Connecting | Connected | Reconnecting | Exhausted`) when
`consumeAsClient=true`, registers a `ConnectionStateCheck` implementing the
PUBLIC `AsyncHealthCheck` trait directly (NOT the private `WsHealthProbe`
internal adapter — the registry accepts `Arc<dyn AsyncHealthCheck>`):
passive, reads the watch, opens NO probe connections; Connected → healthy,
other states → unhealthy with the state name. The watch sender is stored on
`WsEndpoint` — the only object spanning both calls — and threaded into
`WsClientConsumer` at `create_consumer`. Server mode keeps registering the
existing `WsHealthCheck` exactly as today.

**Metrics** (ADR-0066 ownership, kafka-consumer precedent):
- Connect attempts + exhaustion error: owned SOLELY by
  `retry_async_cancelable(metrics=Some(...))` — per-attempt
  `increment_retry_attempt("ws","connect")`, one exhaustion
  `increment_errors("connect","e:ws:connect")`. Call site adds nothing on
  `connect` (D6/D13).
- Frame outcomes: through the `ComponentMetrics` FACADE —
  `runtime.component_metrics().observe("ws", "frame", failed)` — so the
  success series flows only under the `[observability.metrics].components`
  lever while failures always reach the error family (never raw
  `record_component_operation` on the collector; metric-labels lint
  enforced).
- Oversized-frame drop and dispatch failure:
  `increment_errors(route_id, "ws_client_consumer")` (ADR-0012 category
  (b′) error family — distinct from the operation family). Exchange
  counting stays the pipeline's job.

**otel.** Consumer `start()` has no Exchange, so producer-style
`inject_from_exchange` handshake injection is impossible and is NOT attempted.
Received exchanges carry fresh tracing context; remote-parent propagation
from inbound frames is out of scope. `cfg(feature = "otel")` code paths must
compile and pass tests in the all-features gate (no conditional dead code).

**Security context.** `set_security_context` stores the context; client mode
adds no inbound trust decision. TLS uses the same `connect_async` client path
as the producer (system/native roots).

**Concurrency model.** `Sequential` (the default) — a single ordered
stream: frames are WebSocket-ordered, so per-exchange spawn machinery
(`Concurrent { max: Some(1) }`) would add overhead without semantic gain.

## Affected crates

- `crates/components/camel-ws` — config, endpoint wiring, new
  `WsClientConsumer`, `ConnectionStateCheck`, tests, CONTEXT.md, README.
- `camel-component-api` — NO changes (retry_async_cancelable,
  ConsumerContext.cancel_token/mark_ready/sender, WsHealthProbe-equivalent
  surface all exist).
- CONTEXT-MAP.md — one key term (`WsClientConsumer`), same change.

## Architecture boundaries

Components layer only; no Runtime, DSL, or Services changes. The DSL `from:`
needs nothing new — plain URI parameter.

## Phases

Single-phase. One coherent subsystem (camel-ws), tasks ordered:
config/metadata → consumer impl → endpoint wiring → health/metrics →
integration tests → docs.

## Alternatives considered

- **Mode flag inside `WsConsumer`** — rejected: struct carries server-only
  machinery; a flag doubles branch count for zero reuse gain.
- **`role=client-consumer` param** — rejected: third role vocabulary where a
  boolean suffices; `consumeAsClient` is Java-parity spelling, one concept.
- **Producer-side receive loop re-injecting exchanges** — rejected: breaks the
  Tower `Service<Exchange>` one-to-one contract; consumers are the only
  sanctioned exchange sources per CONTEXT-MAP.
- **Sidecar/WASM plugin** — rejected: the workaround this change eliminates.
