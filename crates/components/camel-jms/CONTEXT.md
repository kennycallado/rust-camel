## Scope and architecture

camel-jms delegates JMS protocol work to a Java bridge process over gRPC through
`BridgeServiceClient`. `ensure_binary` resolves the bridge binary from its
version and cache directory. `JmsBridgePool` manages one `BridgeSlot` for each
broker. It monitors health with periodic gRPC `HealthRequest` calls and restarts
failed bridges with exponential backoff. The backoff has a 120-second cap and
has unlimited attempts by default. `bridge_create_lock` and `max_bridges`
serialize and limit bridge admission.

### Connection lifecycle

A slot moves from `Starting` to `Ready { channel }`. A failed health probe moves
it to `Degraded(reason)`, then to `Restarting { attempt, next_at }`. A restart
returns it to `Ready` or moves it to `Stopped`.

`LazyJmsProducer::poll_ready` reflects the slot state. It returns `Ready(Ok)`
for a ready or unscheduled slot. It returns `Pending` during `Starting` or
`Restarting` and registers the task waker. It returns `CamelError::ConsumerStopping`
for `Stopped` and `CamelError::ProcessorError` for `Degraded`. The inner
`JmsProducer::poll_ready` returns `Ready(Ok(()))` unconditionally. The `call()`
future acquires the concurrency semaphore permit. A closed semaphore surfaces
`ConsumerStopping` from `call`, not from `poll_ready`, as ADR-0024 specifies.

### Consumer shutdown

`JmsConsumer::stop` follows ADR-0007. It cancels the `CancellationToken` and
allows five seconds for all `task_handles` to finish. It then aborts tasks that
remain. `consumer_loop` observes cancellation through `tokio::select!`. An
in-flight `ConsumerContext::send` completes before the loop checks cancellation
again. The Consumer does not restart itself. Route supervision owns restart.

## Trust boundary and credential redaction

### Message data

Incoming JMS headers, bodies, correlation IDs, and destinations enter
`exchange.input` without validation or redaction. ADR-0032 assigns validation
to the route where data enters a control action, resource decision, or
executable or interpretable sink.

### Bridge credentials

camel-jms does not use JNDI to resolve broker connections. Broker configuration
crosses an explicit gRPC boundary to the Java bridge process. The process path
comes from `ensure_binary`, which uses the configured bridge version and cache
directory.

`BrokerConfig` implements `Debug` manually and replaces its password with
`<redacted>`. `BridgeSlot` also implements `Debug` manually and omits its
`credentials` field. `LazyJmsProducer` does not implement `Debug`. The bridge
process receives the username as plain text and the password through
`Redacted::new`. `redact_url` masks URL user information as `***@`, drops
the query and fragment behind `?[redacted]` / `#[redacted]` sentinels, and
caps the output at 256 bytes before logging. `BrokerConfig` Debug renders
`broker_url` through `redact_broker_url` (`camel_api::redact::
redact_url_with_query_allowlist`): sensitive keys redact per key
(credential-shaped keys render as a bare `<redacted>` — the key never
echoes, ADR-0076 appendix §2), benign keys stay visible for ActiveMQ
failover diagnosis, and values that decode
to credential shapes (`user:pass@host`) mask as `<redacted>`.
Tests `broker_config_debug_redacts_password` and
`redact_url_strips_userinfo_with_password` verify these properties.

Do not derive `Debug` for a type that contains a password, username,
credential, secret, or token. Implement `Debug` manually and redact or omit
each sensitive field. Use this audit command to check the crate:

```text
rg '#\[derive.*Debug' crates/components/camel-jms/src/
```

### Lifecycle boundary

Bridge restart stays inside `JmsBridgePool`. Consumer task failure remains
route-supervised. `JmsConsumer::stop` cancels, joins with a five-second grace,
and then aborts remaining tasks. This shutdown sequence complies with ADR-0007.

## Trace context propagation

The `otel` cargo feature (optional `camel-otel` dependency, camel-http
pattern) enables W3C trace-context injection on bridge RPCs. When enabled,
`bridge_service_client` wraps the channel with `TraceContextInterceptor`
(`trace_context.rs`), which attaches the ambient camel-otel context as
`traceparent` (+ `tracestate`) gRPC metadata on every RPC (Send, Subscribe,
Health). The producer additionally injects the Exchange's `otel_context` on
Send so real route traces propagate and the exchange-scoped context wins over
the ambient one. An existing `traceparent` is never overwritten; entries that
are not valid gRPC metadata are skipped. With no active context the
interceptor injects nothing and the RPC keeps existing semantics (no-tracing
fallback). The observability delta spec
`openspec/changes/bridge-otel-observability/specs/observability/spec.md`
defines this contract.

## Log-level policy

Per ADR-0012:

- **(c) system-broken** (component.rs L463): bridge restart max-attempts exhausted → "staying degraded". Component-layer lifecycle termination. Keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error!).

- **(b′) outside-contract** (consumer.rs L316): normal-data ctx.send to pipeline failure. `runtime.metrics().increment_errors(route_id, "b-prime:jms:consumer-send")` BEFORE `error!`. Keeps `error!`. Source read confirmed no deliberate error-handoff — `build_exchange(&jms_msg, map_headers)` constructs Exchange from real JMS message, no `set_error()` or `bridge_error_handler` wrapping.

Reviewer: r_glm5.1 verifies these classifications against source at Phase C review time.
