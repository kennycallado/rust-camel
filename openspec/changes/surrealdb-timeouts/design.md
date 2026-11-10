# Design: surrealdb-timeouts

## Approach

surrealdb 4.0.0 (pinned in `Cargo.lock`, workspace features `protocol-ws`,
`protocol-http`, `rustls`, `kv-mem`) builds its reqwest client internally —
`ClientBuilder::new().default_headers(..).resolve(..)` with no `.timeout`
(`engine/remote/http/native.rs:65`); the WS engine has no timeout either, and
`any::connect(address)` exposes no options. Verified against the registry
source of the pinned version. Wrapping with `tokio::time::timeout` at the
component boundary is the only lever.

Two bounds, configured as datasource `extra` keys in the component's existing
extras style (plain lowercase keys, like `namespace`/`password`):

- `connect_timeout_ms` — default 10000. Bounds one connect attempt (the
  SDK call; the attempt-count retry around it is unchanged).
- `operation_timeout_ms` — default 30000. Bounds each post-connect SDK
  round trip it can reach (signin, ns/db selection, producer statements,
  live subscription).

Both are connection/server concerns, so they live on the datasource (which
already owns `db_url` and credentials), not on the endpoint URI. Values are
TOML integers in milliseconds, parsed by a `SurrealDbTimeouts` helper next to
the existing `extra_str` helpers in `pool_factory.rs`; zero, negative,
empty, or non-numeric values are rejected with a message
naming the option. No upper cap: an operator who wants a very long bound
states it deliberately.

### Where the configuration reaches (storage)

The catalog stores the raw SDK client (`Arc<Surreal<Any>>`) and the
`check`/`close` hooks receive only a `DatasourceHandle` — no configuration.
Keeping that payload type avoids cross-crate downcast changes (the scenario
tier's `resolve_datasource::<Surreal<Any>>` in `camel-integration-test`
depends on it). Consequence, stated explicitly:

- Sites 1–4 (factory `create`, which holds `DatasourceConfig`) parse the
  extras and use the configured values.
- Sites 7–8 (producer dispatch, live subscribe) receive configured values
  through `SurrealDbEndpointConfig` fields (defaults in `Default`, mirroring
  how `retry: NetworkRetryPolicy` already rides the config struct);
  `create_endpoint` parses the datasource extras and overrides the defaults,
  so invalid values fail fast at route creation and the values reach
  producer, consumer, and polling through the existing `config.clone()`
  threading.
- Sites 5–6 (health check, `invalidate()` teardown) use the built-in
  `DEFAULT_OPERATION_TIMEOUT` constant (30s). The hooks' signatures carry no
  configuration, so the extras do not apply there; the change still removes
  their unboundedness. Documented limitation in docs and CONTEXT.md.

Evidence for the values: workspace precedent is 10s connect / 30s total
(keycloak client hardening, audit row 3; otel exporter default 10s, row 22).
Worst-case connect budget becomes ~10 attempts × 10s + ~81s capped backoff
(the default `NetworkRetryPolicy`: 10 attempts, 100ms→30s) instead of
infinite.

### Error surface

New `SurrealDbError::Timeout { phase: &'static str, deadline_ms: u64 }`
variant; Display text names phase and bound in milliseconds, e.g.
`surrealdb connect exceeded the 10000ms deadline` or `surrealdb statement
execution exceeded the 30000ms deadline`. A timeout is not a protocol error:
no `#[from]` SDK source, it maps through the existing `From<SurrealDbError>`
`_` arm to `CamelError::ProcessorError`. `is_retryable` stays false for
timeouts: a timed-out connect attempt already consumed its full deadline, so
re-arming the retry would stack 10 × deadline on a stalled server; the retry
policy itself (counts, backoff, ADR-0013) is unchanged and still re-arms for
fast transient failures (refused, DNS, TLS drop).

Setup retry preservation: the setup `retry_async` is typed
`retry_async<_, _, _, _, surrealdb::Error>` and classifies through
`is_transaction_conflict`. The per-call timeouts keep that signature: each
wrapped setup call maps deadline expiry to
`surrealdb::Error::internal("surrealdb {phase} exceeded the {deadline_ms}ms
deadline")`. An internal message is not a transaction conflict, so the
classifier still returns false and setup timeouts fail fast; the honest
phase+bound text survives into the outer ProcessorError (redacted URL
prefix). The producer/consumer elapsed paths use the component
`Timeout` variant, not a protocol variant. There is no existing producer
statement timeout path — this change adds the first one.

## Call sites (every SDK call the component owns)

| # | Site | Phase | Bound |
|---|------|-------|-------|
| 1 | `pool_factory.rs` `connect(endpoint)` (inside per-attempt closure) | connect | `connect_timeout_ms` (configured) |
| 2 | `pool_factory.rs` `signin(Root{..})` | signin | `operation_timeout_ms` (configured) |
| 3 | `pool_factory.rs` `use_ns(&ns)` | namespace selection | `operation_timeout_ms` (configured) |
| 4 | `pool_factory.rs` `use_db(&db)` | database selection | `operation_timeout_ms` (configured) |
| 5 | `pool_factory.rs` `query("INFO FOR DB")` (health check) | health check | built-in 30s constant (handle carries no config); expiry → `HealthStatus::Unhealthy` |
| 6 | `pool_factory.rs` `invalidate()` (close hook) | teardown | built-in 30s constant; expiry → error return |
| 7 | `producer.rs` `execute` operation dispatch (query/select/create/update/upsert/delete/patch/relate/vector/search/run) | statement execution | `operation_timeout_ms` (configured) |
| 8 | `consumer.rs` `select(..).live().await` | live subscription | `operation_timeout_ms` (configured) |
| 9 | `polling.rs` `receive()` query | polling query | caller-supplied `receive(timeout)` (ADR-0015) — unchanged |

In `execute`, client resolution (`resolve_client` → pool creation) stays
outside the wrapper so pool-establishment stalls surface with their own phase
names (sites 1–4), and only the statement dispatch is wrapped as site 7.

## Cancellation semantics

`tokio::time::timeout` drops the inner future at the deadline; the caller is
released. Dropping does NOT cancel server-side work on either engine:

- HTTP engine: the router loop spawns a detached task per request
  (`tokio::spawn` in `engine/remote/http/native.rs`); the request continues,
  and its response is sent into a channel no one reads.
- WS engine: the request bytes may already be on the wire; the server can
  finish the statement as orphaned work.

The SDK exposes no cancellation point. Local cancellation is all the component
claims; no server-side cancellation is promised anywhere. The router task
survives dropped futures, so later calls on the same client proceed — stated
as an implementation observation, not a contract.

## Unboundable phase (residual)

Live notification delivery after the subscription (site 8's stream loop) is
long-lived by design; a per-operation deadline cannot apply without killing
healthy subscriptions. A silently stalled WS (no frames, no reset) is
undetectable without a heartbeat/watchdog. Out of scope here — filed as bd
`rc-lft0b` (`discovered-from: rc-0p04f`), not silently claimed.

## Affected crates

- camel-component-surrealdb: all changes (parse helper + endpoint-config
  fields, error variant, factory/producer/consumer wrapping, tests).

## Architecture boundaries

Component-local; no camel-api/camel-core contract change, no change to the
stored catalog payload type (the integration tier's
`resolve_datasource::<Surreal<Any>>` is untouched). The data/control plane
split is respected: timeouts bound data-plane SDK awaits inside the
component; no control-plane decision reads them. ADR-0013 retry semantics
untouched; ADR-0015 polling contract untouched; ADR-0012 log levels: the
timeout surfaces as an error result, no new `error!` site. The `mem://`
scheme semantics are unchanged (embedded connect completes in-process; the
bounds apply but never trip healthy paths). Datasource extras are
operator-config (trusted, ADR-0032); they only bound durations.

## Tests

Parse/validation (pure, in-crate): defaults applied when extras are absent;
explicit values used; TOML integer accepted; zero, negative, empty, and
non-numeric rejected naming the option (both keys).

Behaviour: a `ws://` endpoint against a listener that accepts and never
answers trips the connect bound at `connect_timeout_ms=100` with the named
error (fast: non-retryable, one attempt). Timing assertions are
scheduler-aware: the test asserts the error variant and text and that at
least the deadline elapsed, under a generous outer guard. A normal `mem://`
producer query with defaults succeeds (wrapper transparency). The elapsed
arms for statements and live subscriptions run through the shared
production wrapper `bound_op` with `std::future::pending()` futures — the
same code path `execute` and `subscribe_live` use, not a formatting-only
assertion. The health/teardown elapsed arms are unit-covered with
future-parameterized helpers (`std::future::pending()` drives the elapsed
branch — a mem client never stalls). No new container test is added: the
existing CI harness (`cargo test -p camel-test --test surrealdb_test
--features integration-tests`, Docker) proves happy-path statement
behaviour only.

## Alternatives considered

- SDK client/timeout hook — does not exist in 4.0.0 (verified); rejected.
- Endpoint URI params for the bounds — the factory sees only
  `DatasourceConfig`, so the connect bound could not live there; two surfaces
  for one concept rejected.
- One single bound for both phases — connect wants a shorter budget than
  statements; merging them either starves connect or bloats statements.
  Two bounds, both defaulted, kept.
- Retryable connect timeout — stacks 10 × deadline on a stalled server;
  rejected (stated as behaviour change in docs).
- Storing a component-local handle (client + timeouts) in the catalog to make
  health/teardown configurable — rejected: changes the stored payload type,
  breaking the scenario tier's `resolve_datasource::<Surreal<Any>>`
  downcast in `camel-integration-test` for a marginal gain (those two hooks
  keep the 30s constant).
- Mapping setup-timeout into the component `Timeout` variant inside the setup
  `retry_async` — rejected: the closure is typed `surrealdb::Error`; changing
  the type would rewrite the classifier plumbing for no behavioural gain.
  The honest text travels in the message.
