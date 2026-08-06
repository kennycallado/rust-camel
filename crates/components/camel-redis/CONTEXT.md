## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (consumer.rs L202, L207, L420):
  - L202 = consumer task returned error during `stop()` shutdown lifecycle.
  - L207 = join error during `stop()` shutdown lifecycle.
  - L420 = retry-exhaustion: max transient-error attempts exceeded, consumer `return Err` ends route lifecycle.
  Each site keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).

- **(b′) outside-contract** (consumer.rs L308, L404):
  - L308 = PubSub `ctx.send()` failure (channel closed). Calls `runtime.metrics().increment_errors(route_id, "b-prime:redis:pubsub-channel-closed")` BEFORE the `error!`.
  - L404 = BLPOP `ctx.send()` failure (channel closed). Calls `runtime.metrics().increment_errors(route_id, "b-prime:redis:blpop-channel-closed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.

- **(e) outside-contract** (consumer.rs L443):
  - L443 = per-message non-transient Redis error. Calls `runtime.metrics().increment_errors(route_id, "e:redis:message-non-transient")` BEFORE the `error!`.
  `error!` at this site stays because the error is per-message and non-recoverable without user action.

Line numbers can drift between revisions. The inline `// log-policy: <category>`
annotation before each `error!` and `scripts/xtask/allowlist-log-levels.txt` are
authoritative under ADR-0012. This section is a readability aid.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.

## Crash health ownership

Per the project-wide contract (CONTEXT-MAP "Supervision / ConsumerRestart"):
the Redis consumer does NOT call `force_unhealthy_for_route` on task crash.
The Runtime pins health via `CrashNotification → RuntimeCommand::FailRoute →
commands.rs`.

## Batch 6 — Security hardening

### `effective_tls()` (config.rs:346)

Auto-enables TLS for non-loopback hosts when `tls=false`. Logic in `effective_tls()`:
- Returns `true` if `tls` is explicitly true, OR host is not `localhost`, `127.*`, `::1`, or `0.0.0.0`.
- Triggers a `tracing::warn!` at runtime (`build_url()`, `apply_defaults()`) reporting the auto-enable.
- `validate_tls()` returns `Config` error if the `redis` crate lacks a TLS feature (`tls-rustls-*` or `tls-native-tls`).

### `connection_timeout_secs` (config.rs:253)

Default: 10 seconds (in `RedisConfig::default()`). Applied at 4 connection sites via `tokio::time::timeout`:
- **Health check** (health.rs:48): `get_multiplexed_async_connection()` wrapped in `connection_timeout`.
- **Producer** (producer.rs:248): lazy `get_multiplexed_async_connection()` wrapped in `connection_timeout`.
- **Consumer PubSub** (consumer.rs:257): `get_async_pubsub()` wrapped in `connection_timeout`.
- **Consumer Queue** (consumer.rs:342): `get_multiplexed_async_connection()` wrapped in `connection_timeout`.

### Health check outer timeout (health.rs:91)

Derived from `connection_timeout_secs + 5` seconds. The outer timeout at `check()` (health.rs:108)
must exceed the inner connection timeout so the inner fires first with a specific error message.
