# Proposal: surrealdb-timeouts

## Why

The surrealdb data plane has no clock. `SurrealDbPoolFactory` calls
`connect(endpoint)` (`pool_factory.rs:150-161`, schemes `ws/wss/http/https/mem`,
`pool_factory.rs:257-259`) with no timeout, and the signin/use_ns/use_db setup,
the health check, `invalidate()`, every producer statement, and the live
subscription are equally unbounded. surrealdb 4.0.0 (pinned in `Cargo.lock`)
builds its reqwest client internally (`ClientBuilder::new().default_headers(..)
.resolve(..)` — no `.timeout`; `engine/remote/http/native.rs:65`) and exposes no
client or timeout hook through `any::connect`, so the SDK offers no lever. A
stalled SurrealDB endpoint holds a route task forever. Only the attempt-count
retry wraps connect, and it re-arms indefinitely on an endpoint that accepts TCP
and never answers.

Verified against the pinned SDK source in the cargo registry: zero `timeout`
references in the native HTTP and WS engines; `any::connect(address)` takes no
options. Wrapping at the component boundary with `tokio::time::timeout` is the
only lever. This fixes the component-side residual of bd `rc-0p04f`
(discovered-from the `rc-s8itn` hardened-client audit, row 29).

## What Changes

- Two new datasource `extra` options, in the component's existing extras style:
  `connect_timeout_ms` (default 10000) and `operation_timeout_ms` (default
  30000). Zero, empty, or non-numeric values are rejected at parse time with a
  message naming the option.
- Bounds on every SDK call site the component owns: connect (per attempt),
  signin, use_ns, use_db, the `INFO FOR DB` health check, `invalidate()`,
  the producer statement dispatch, and the live subscription. The health
  check and teardown use the built-in 30s default (their hook signatures
  carry no configuration); the other sites honor the configured extras. The
  polling consumer keeps its caller-supplied `receive(timeout)` bound
  (ADR-0015).
- New `SurrealDbError::Timeout { phase, deadline_ms }` variant with honest
  text ("surrealdb connect exceeded the 10000ms deadline"). A timeout is not
  a protocol error and is not retried: a timed-out attempt already consumed
  its deadline.
- Docs: `docs/src/components/surrealdb.md`, crate `README.md`, crate
  `CONTEXT.md` gain the options, defaults, and what each bound covers.
- Spec delta: new `surrealdb-timeouts` capability (no existing capability owns
  this component; `cache-repo-configuration` is unrelated).

Excluded (hard limits): no dependency change, no SDK fork, no retry count or
backoff change, no pool-sizing change, no `mem://` semantics change, no other
components.

## Acceptance criteria

- A stalled endpoint (listener that accepts and never answers) trips the
  connect bound with an error naming the phase and the bound.
- A normal query against the embedded `mem://` instance is unaffected.
- Defaults bound: with no extras configured, connect trips at 10s and
  operations at 30s (today: unbounded) — behaviour change stated in docs.
- Zero/empty/non-numeric extra values fail at parse time naming the option.
- `cargo test -p camel-component-surrealdb` green; local gates green.

## Risk budget

- Defaults are a behaviour change (unbounded → bounded). Accepted: an
  operation that outlasts 30s indicates a stalled or pathologically
  overloaded server, and a hanging route is strictly worse. Operators can
  raise the values.
- Timeout expiry releases the caller only; the SDK keeps executing the
  orphaned request server-side on both engines (no cancellation point).
  Stated in design and docs; not claimed as server-side cancellation.
- Statement-phase and subscription-phase timeouts against a stalled live
  server cannot be reproduced locally or by the existing CI harness (which
  proves happy-path statement behaviour only); no new container test is
  added. The elapsed arms are unit-covered (future-parameterized helpers
  with `std::future::pending()`). Local proof covers the parse rules, the
  connect trip, and wrapper transparency.
- Live notification delivery after the subscription stays unbounded by
  design; a silently stalled WS cannot be detected without a heartbeat.
  Residual filed as bd `rc-lft0b`.

Bd: rc-0p04f
