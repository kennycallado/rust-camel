# NetworkRetryPolicy and Retry Migration Boundaries

Status: Accepted

## Context

Components duplicated capped exponential backoff loops for transient network reconnects, each with its own config struct, classifier, and log shape. The duplication made retry behavior inconsistent (max_attempts=0 meant unlimited in some components and zero-attempts in others), backoff hard to tune, and reconnect observability noisy.

`NetworkRetryPolicy` in `camel-component-api` provides the shared config (max_attempts, initial_delay, multiplier, max_delay, jitter), shared defaults, and a single canonical classifier for `CamelError` (`is_retryable_camel_error`). Two execution helpers sit on top: `retry_async` for bounded retry of pure async operations, and `retry_async_cancelable` for the same operations when route or consumer shutdown must interrupt the inter-retry sleep.

Some consumers cannot be expressed as a single closure passed to these helpers. They borrow mutable caller state across `await` points, span polling or event-stream loops, need async lifecycle side effects between attempts (bridge restart, pool slot restart), share an attempt counter across multiple recovery sites, or must not resend non-idempotent writes after transport failure.

## Decision

Provide two public retry primitives in `camel-component-api::network_retry`, plus the
`is_retryable_camel_error` classifier:

- `retry_async(policy, label, op, is_retryable)` — bounded retry of `op: FnMut() -> Future<Output = Result<T, E>>`. `label: Option<&'static str>` emits a component identifier as a structured `component` tracing field (e.g., `"ws-producer: transient error — retrying"`). Pass `None` for the unlabeled path. Cancels only via the policy's `max_attempts`.
- `retry_async_cancelable(policy, label, op, is_retryable, &CancellationToken)` — same as `retry_async`, plus cancellation honored during inter-retry sleep. Cancel-during-sleep returns the last operation error (no synthetic `Cancelled` variant). Cancel during `op` itself is the caller's responsibility.
- The former `retry_camel_error` convenience wrapper was removed (zero non-test consumers); callers whose error type is `CamelError` use `retry_async(policy, label, op, is_retryable_camel_error)` directly.

Do not add a public stateful HRTB/boxed-future variant (`retry_async_with_state`). The confirmed Redis producer and executor cases borrow mutable state across `await`, but they also require retry-specific side effects (`reconnect`, stale connection clearing) whose timing is clearer in explicit loops. These sites should keep manual loops using `NetworkRetryPolicy::should_retry` and `NetworkRetryPolicy::delay_for`.

## Migration decision tree

- **Use `retry_async`**: operation owns or clones its state, no shutdown-cancellation need. Examples: WS connect (`Some("ws-producer")`), CXF pool connect (`Some("cxf")`), gRPC producer (`Some("grpc-producer")`), OpenSearch producer (`Some("opensearch-producer")`), SQL pool connect (`Some("sql-consumer")`, `Some("sql-producer")`).
- **Use `retry_async_cancelable`**: same conditions as `retry_async`, but route/consumer shutdown must interrupt backoff. Example: Container Docker connect (`Some("container-events")`, `Some("container-logs")`).
- **Stay manual**:
  - Operation borrows mutable caller state across `await` (e.g., `&mut Exchange`, `&mut Executor`) or needs pre-retry side effects tied to that state. Do not wrap this in a public HRTB/boxed-future helper; keep the retry loop explicit and use `NetworkRetryPolicy::should_retry` / `delay_for` for shared policy semantics. Examples: Redis producer, Redis executor.
  - Retry spans a polling or event-stream loop that runs until cancelled, not bounded attempts. Example: Kafka consumer `recv()` loop, SQL consumer polling loop, Container event stream loop.
  - Retry needs async pre-retry lifecycle side effects (e.g., `restart_bridge().await?`, `pool.restart_slot().await?`) between attempts. Example: XSLT and XJ bridge restart, JMS consumer reconnect.
  - Retry counter is shared across multiple recovery sites in the same loop. Example: JMS consumer's three nested retry sites sharing one `attempt` counter.
  - Resend after transport error would duplicate non-idempotent writes. Example: JMS producer.

## Consequences

- Shared retry behavior for simple network reconnects. Consistent semantics for `max_attempts` (1 initial + N-1 retries; `0` = unlimited), backoff shape, jitter, and log fields.
- Manual loops remain intentional, each with a comment explaining which branch of the decision tree excluded it from the shared helpers.
- Follow-up work tracked in bd:
  - `rc-cvq` was evaluated and closed wontfix: a stateful HRTB/boxed-future helper would add public API complexity without simplifying the confirmed Redis sites enough to justify it.
  - `rc-1nm` resolved: `retry_async` and `retry_async_cancelable` now accept `label: Option<&'static str>`. All 8 migrated components pass component identity via `Some("ws-producer")` etc., emitting a structured `component` tracing field. Operators can filter by component label.
  - Container test dead-store cleanup (pre-existing `attempt = 0` before `break`).
