# camel-component-kafka — CONTEXT

## Log-level policy

Per ADR-0012, this component's `error!`/`warn!` sites are categorized as:

### (b') outside-contract

Each site calls `runtime.metrics().increment_errors(route_id, "b-prime:kafka:<site>")`
BEFORE the `error!` macro. The metric is the operator signal; `error!` provides
loud log visibility. Both stay.

- **Auto-commit dispatch** (`consumer.rs` `auto_commit_step` helper):
  `send_and_wait` returns `Err` on normal-data. The route handler did not
  absorb the failure. Metric: `b-prime:kafka:auto-commit-dispatch`.
- **Auto-commit side-effect** (`consumer.rs` `auto_commit_step` helper):
  TPL build or `consumer.commit()` returns `Err`. Metric:
  `b-prime:kafka:auto-commit-side-effect`.
- **Manual-commit dispatch** (`consumer.rs` manual branch, UNCHANGED):
  fire-and-forget `ctx.send` channel-closed failure. The auto success-gate
  contract does not bind manual commit timing. Metric:
  `b-prime:kafka:manual-commit-dispatch`.
- **Manual async commit handler — invalid topic/partition** (`consumer.rs`
  commit-handler task ~L363): the commit-handler task failed to build the
  TPL from a `CommitRequest`. Metric: `b-prime:kafka:async-commit-reply`.
- **Manual async commit handler — commit failure** (`consumer.rs`
  commit-handler task ~L377): the `consumer.commit()` call failed in the
  async commit-handler task. Metric: `b-prime:kafka:async-commit-failed`.
  (Previously mislabeled as poll-path ctx.send — corrected post-Q1.)

### (c) system-broken

Each site keeps `error!` with `// log-policy: system-broken`. No metric
(operator alert via `error!` is the signal).

- `bundle.rs` `register_all` startup-time bundle init failure (lifecycle
  failure, no route_id, no runtime accessor).
- `consumer.rs` recv-loop "exhausted reconnect attempts" — consumer `break`
  ends the route lifecycle.
- `consumer.rs` `stop()` task-join errors (~L143-158, shutdown path, NOT
  ctx.send). Reclassified post-Q1 from a prior incorrect (b') label.

### (a) handler-owned

- `producer.rs` `call()` returns `Err` — the pipeline catches the failure
  and the route `ErrorHandler` owns the operational signal. Downgraded to
  `warn!` with `// log-policy: handler-owned`. No metric.

### Sites explicitly NOT in the ADR taxonomy

- `consumer.rs` commit-handler drain warnings during manual shutdown are
  operational warnings, not ADR error sites. Leave at `warn!`/`info!`.

## Crash health ownership

Per the project-wide contract (CONTEXT-MAP "Supervision / ConsumerRestart"):
the Kafka consumer does NOT call `force_unhealthy_for_route` on task crash.
The Runtime pins health via `CrashNotification → RuntimeCommand::FailRoute →
commands.rs`. The consumer's `runtime` field is metrics-only.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C
review time.

## Batch 6 — Security hardening

### SSL/SASL feature gates

SSL and SASL protocols are gated behind cargo features to control OpenSSL linkage:

- `ssl` — enables `security_protocol=SSL` via system OpenSSL (requires `libssl-dev` / `openssl-devel`)
- `ssl-vendored` — same as `ssl` but bundles OpenSSL statically (no system deps, larger binary)
- `sasl` — enables `SASL_PLAINTEXT` and `SASL_SSL` protocols (implies `ssl`)

Validation in `check_feature_gates()` (config.rs) enforces that `ssl` or `ssl-vendored` is active
when `security_protocol` is `SSL` or `SASL_SSL`, and that `sasl` is active for `SASL_PLAINTEXT`
or `SASL_SSL`. Missing features produce a clear startup error with the required `cargo add` command.

### Plaintext protocol warning

The `validate()` method (config.rs:240-248) emits `tracing::warn!` when `security_protocol` is
`Plaintext` or `SaslPlaintext`. Message: "Kafka using cleartext protocol — credentials and data
in plaintext". The warning is advisory only — validation succeeds regardless. Both `KafkaConfig`
(global defaults) and `ResolvedKafkaEndpointConfig` (per-endpoint) call `validate()`.
