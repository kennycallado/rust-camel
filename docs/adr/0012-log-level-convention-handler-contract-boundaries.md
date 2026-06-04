# Log-Level Convention: Handler-Contract Boundaries

Adopt a project-wide log-level convention based on **handler-contract boundaries**: emitters *inside* a contract boundary where a downstream handler will own the failure (taxonomy categories **(a)** Producer/Processor inside a pipeline, **(b-bridged)** Consumer→pipeline with `bridge_error_handler=true`) MUST log at `warn!` or below; emitters *outside* any handler contract (categories **(b′)** consumer side-effect failure after `send_and_wait`, **(g)** unrecoverable Producer/Endpoint creation failure, **(e)** transient server accept/retry loops) MAY log at `error!` but MUST pair the call with a signal replacement — `MetricsCollector::increment_errors(route_id, "<category>:<component>:<site>")` for (b′)/(e), `HealthCheckRegistry::force_unhealthy_for_route(route_id, name, reason)` for (g); emitters in **system-broken** categories — corruption, panic-equivalent, contract violation (**c** supervisor, **d** CLI/bootstrap, **f** corruption, **h** pre-pipeline authz fault) — MUST log at `error!` with no replacement required. The convention is enforced by `xtask lint-log-levels` requiring every `error!` in non-test code to carry one of three preceding annotations: `// log-policy: system-broken | outside-contract | handler-owned` (the last forbidding `error!` outright).

## Background

A prior audit proposed a blanket downgrade of `error!` → `warn!()` across components that returned errors via `Result<_, CamelError>`. Rejected: the same emitter may be used both by routes with a configured handler (downgrade safe) and by routes without one (downgrade loses the only operational signal). Apache Camel's documented behavior matches our adopted model: `DefaultErrorHandler` propagates "as if there were no error handler at all" (emitter keeps the ERROR); `Dead Letter Channel` is the single place that logs at ERROR level (handler-owned). See <https://camel.apache.org/manual/error-handler.html>.

## Taxonomy (normative)

| Code | Category | Inside handler contract? | Required level | Signal replacement |
|------|----------|:------------------------:|:--------------:|--------------------|
| (a)  | Pipeline Processor/Producer invocation | Yes | `debug!` or `warn!` | None — handler owns |
| (b-bridged) | Consumer **deliberately** hands an error-bearing Exchange to the route handler (e.g., `bridge_*_error`, `set_error` + `send_and_wait` with the sole purpose of invoking the handler) | Yes | `debug!` or `warn!` | None — handler owns |
| (b′) | Consumer side-effect failure, including a **normal-data** `send_and_wait → Err` (the route did NOT absorb it — see "b-bridged discriminator" below) | No | `warn!` or `error!` | `increment_errors(route_id, "b-prime:<component>:<site>")` required if downgraded |
| (g)  | Endpoint/Producer creation failure (permanent) | No | `error!` (preferred) | `force_unhealthy_for_route(route_id, "endpoint-creation", reason)` if downgraded |
| (e)  | Server-loop accept/retry transient (grpc, container, http) | No | `warn!` | `increment_errors(route_id, "e:<component>:<site>")` required if downgraded |
| (c)  | Route lifecycle: start/stop/suspend/resume (`consumer_management.rs`, `route_controller.rs`) | System-broken | `error!` | None |
| (d)  | CLI, bootstrap, application startup/shutdown | System-broken | `error!` | None |
| (f)  | ControlBus dual-cut (origin side) | System-broken | `error!` | None |
| (h)  | Pre-pipeline authorization/policy evaluation fault | System-broken | `error!` | None |

## Tie-breaker

Ask: *"Does this error path produce an Exchange that flows into a Route pipeline with an `ErrorHandlerLayer`?"* If yes → INSIDE → categories (a)/(b-bridged) → emitter logs ≤ `warn!`. If the error stays inside consumer/server/lifecycle and only flows to `CrashNotification` / log / supervisor → OUTSIDE → categories (b′)/(g)/(e)/(c)/(d)/(f)/(h) → emitter follows the table above.

## b-bridged discriminator (normative)

The `b-bridged` category applies **only** when the consumer constructs an error-bearing Exchange for the **sole purpose** of invoking the route error handler — i.e., the consumer decides "this failure should be treated as a route-level exchange failure" and explicitly bridges it.

The discriminator is **NOT** "did the call return `Err`?". Read the actual contract:

- `ConsumerContext::send_and_wait` (`crates/components/camel-component-api/src/consumer.rs:77-91`) documents that `Err` is returned *"if the pipeline failed **without** an error handler absorbing the error"*.
- `error_handler.rs::send_to_handler` (lines 248–276) returns `Ok(exchange)` in **every** branch (no-handler-configured, handler-not-ready, handler-call-failed) **and** logs `error!` itself in those branches.

Consequences:

1. **Handler absorbed → `send_and_wait` returns `Ok`** → the `if let Err(...)` emitter branch never fires. The route handler (or `error_handler.rs` itself) is the single ERROR owner.
2. **`send_and_wait` returns `Err` → no handler absorbed it** → `error_handler.rs` logged nothing at ERROR → the consumer's `if let Err(...)` is the **only** ERROR signal for that failure. Downgrading it to `warn!` deletes operator visibility for a genuinely unhandled failure.

Therefore:

| Site shape | Category | Required level |
|------------|----------|----------------|
| `bridge_*_error(...).await` (synthetic Exchange with `set_error` whose only purpose is invoking the route handler) — handler succeeds → caller emits at most `warn!` | **(b-bridged)** | `warn!` |
| `bridge_*_error(...).await` itself returns `Err` (the route did NOT absorb the bridge — e.g., no handler configured) | **(c)** system-broken | `error!` |
| Normal-data `send_and_wait(exchange).await` where `exchange` was constructed from real input (not for error-handoff purposes) and `Err` means the pipeline failed unhandled | **(b′)** outside-contract | `error!` (with `increment_errors` metric when available) |

A regression test at `crates/camel-processor/src/error_handler.rs` (or sibling) MUST assert that an **unbridged / no-handler** `send_and_wait → Err` produces an ERROR-level signal somewhere — i.e., the consumer's emitter MUST stay at `error!` for that path. The test is the **only** mechanical check that the rule above is not silently inverted by a future contributor.

## Signal replacement API constraints

- `MetricsCollector::increment_errors(route_id, error_type)` — existing API at `crates/camel-api/src/metrics.rs:11`. Label value MUST match the regex `^(b-prime|e|g):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$`. Examples: `b-prime:sql:on-consume`, `e:grpc:accept`, `g:http:endpoint-create`.
- `HealthCheckRegistry::force_unhealthy_for_route(route_id, name, reason)` — existing API at `crates/camel-core/src/health_registry.rs:49`. Pins the route to Unhealthy (HTTP 503, pod NotReady). This is correct for category (g): a route without a Producer is non-functional, NOT Degraded. Supervision restart (ADR-0007) clears the pin via `register_for_route` once the endpoint is recreated. There is no `force_degraded_for_route` API and we explicitly do NOT add one — a half-functional route is worse than a removed-from-rotation one.

## Lint annotations

Every `error!(...)` in non-test code MUST be preceded by exactly one of:

```rust
// log-policy: system-broken
error!(...);          // categories (c)(d)(f)(h) — no further requirement

// log-policy: outside-contract
error!(...);          // categories (b′)(e)(g) — REQUIRES on a nearby line:
                      //   metrics.increment_errors(route_id, "<cat>:<component>:<site>")
                      //   OR
                      //   health_registry.force_unhealthy_for_route(route_id, name, reason)
                      //   OR
                      //   a `if !bridged { ... }` guard containing the call

// log-policy: handler-owned
warn!(...);           // category (a)(b-bridged) — MUST NOT be error!
```

Enforced by `xtask lint-log-levels` (pattern at `scripts/xtask/src/main.rs:868` for `lint_unwrap`). Two parallel ratchets:

1. **Allowlist** (`scripts/xtask/allowlist-log-levels.txt`) — explicit per-site `<rel path>:<line>` entries. Paired `allowlist-log-levels.txt.max` enforces monotone-non-increasing post-seed.
2. **Inline escape** (`// allow-log-levels` on the same line as `error!`) — counted across all `src/**/*.rs` files. Paired `allow-inline.max` enforces `count <= max`; migrations adding deferred sites MUST bump `allow-inline.max` atomically in the same commit.

Every inline escape MUST be preceded (within 3 lines) by a `TODO(ADR-0012-<flavor>): ... via bd <id>` marker where `<flavor>` ∈ `{e-metrics, g, ...}` and `<id>` is a live bd chore id. Lint fails if the TODO marker is missing or the bd id is absent. (Liveness of the bd id is enforced by periodic CI job; the lint verifies format only.) This prevents deferred sites from becoming invisible and permanent — every escape is traceable to a deferral chore.

## Migration scope (informational)

Roughly 116 `error!()` sites across `crates/`. Estimated ~25 relevelled, ~6 kept as system-broken, ~3 removed as duplicates of the handler log. First migration wave: `crates/components/camel-sql/` (including the duplicate-`error!` bug at `consumer.rs:429` on the bridged path) plus `crates/components/camel-direct/` (dead `bridge_error_handler` field removal — `direct` is producer-driven synchronous, bridging is semantically incoherent for it; closes `TODO(DIR-005)` as won't-fix).

Two sites were initially mis-categorized as `b-bridged` and corrected after the second-expert review (see `docs/superpowers/reviews/2026-06-04-adr-0012-second-expert-review.md`): `camel-sql consumer.rs:205` (StreamList downstream `send_and_wait`) and `camel-direct lib.rs:296` (consumer `send_and_wait`). Both are **normal-data** sends whose `Err` means the failure was NOT absorbed by the route handler; they are categorized **(b′)** outside-contract and MUST keep `error!` (with `// log-policy: outside-contract` + `increment_errors` metric once `rc-mf3` lands).

## Cross-references

- **ADR-0007** — Route-supervised consumer failure. CrashNotification path is system-broken category (c); retains `error!`.
- **CONTEXT-MAP.md** Key Terms — adds four new terms: Handler-contract boundary, System-broken error, Side-effect failure, Bridged error.
- **Apache Camel** — Error Handler (<https://camel.apache.org/manual/error-handler.html>), Log component (<https://camel.apache.org/components/latest/log-component.html>). `DefaultErrorHandler` propagates "as if there were no error handler at all"; `Dead Letter Channel` is the single ERROR-emitting handler.
