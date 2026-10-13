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

`JmsConsumer::stop` follows ADR-0007. It cancels the requested-shutdown token
first. It then allows five seconds per owned `task_handle`, and aborts any
straggler. Abort authority stays component-owned through `worker_aborts` and the
supervisor drop guard, so no worker is detached. `consumer_loop` races shutdown
against `send_and_wait` and against the completion RPC. A cancel before the
terminal decision drops the pending wait and the subscription without a
speculative acknowledgement, so the broker redelivers. A decision already made
stands. On a task failure, sibling consumers get a bounded three-second drain
before the component aborts them. The Consumer does not restart itself. Route
supervision owns restart.

## Settlement and attempt journal

The bridge session runs with `CLIENT_ACKNOWLEDGE` and holds one unacknowledged
delivery per session. Settlement is completion-driven, not receipt-driven.
`acknowledgementMode` and `transactionMode` do not change it. The Rust consumer
submits with `send_and_wait`. `Ok`, including a handled error and a `Stop`,
sends `COMMIT` and acknowledges once. A propagated `Err` and a closed pipeline
channel send `ROLLBACK` and recover the session. A cancel before the decision
prevents acknowledgement. The same event after the decision does not undo it. A
lost `COMMIT` response stands and is not retried. A healthy slow route has no
completion deadline.

The bridge records each delivery as a durable reservation before handoff. It
caps each `JMSMessageID` at `JMS_MAX_DELIVERY_ATTEMPTS` (default 6). The seventh
receipt is the terminal reject-then-log discard: one acknowledgement and one
`error!` log with the message ID and the count only. The bridge does not forward
to a dead-letter destination.
This discard is the explicit exception to at-least-once, applied to every
message type.

### Journal ownership

Ownership is lazy and per destination. The domain is the tuple of directory,
broker key, and destination key. `JmsBridgePool` never locks a journal. Producer
and topic paths never acquire one. The bridge acquires the lock at the first
queue subscribe and releases it when the last subscription ends. A busy lock
waits a bounded time, then the subscribe fails with `FAILED_PRECONDITION`. A new
owner inherits the recorded counts through lock handoff.

The `resolve_attempt_journal_dir` and `attempt_journal_broker_key` helpers in
`config.rs` derive the Rust-owned directory and key. The component passes them
as `JMS_ATTEMPT_JOURNAL_DIR` and `JMS_BRIDGE_BROKER_KEY`. They are not operator
inputs. Unset values defer the failure to a queue subscribe with
`FAILED_PRECONDITION`.

The cap is not absolute. Compaction retires entries untouched past the
retention window (default 24 hours). The current reservation is never retired,
and no live reservation is evicted. Duplicates across distinct domains are
bounded at N times the cap, where N is the number of domains within one
retention window. Topic subscribers use a per-subscription volatile counter,
which resets on disconnect. Durable topic subscribers are unsupported.

### Operator recovery

On a permanent failure, follow a strict sequence.

1. Stop the route and its context.
2. Confirm that bridge and journal ownership has ended before you edit any file.
   The last subscription for the affected domain must be released, so its
   reference count reaches 0. Wait for the bridge process and its file lock to
   be released.
3. Only then repair or remove the corrupt journal or the unkeyable source for
   the affected domain.
4. Restart the context.

Do not edit a live journal file while a route or bridge holds the lock.

## Producer header allowlist

`JmsEndpointConfig::jms_header_allowlist` holds the endpoint
`jmsHeaderAllowlist` (alias `jms_header_allowlist`). The producer forwards an
outgoing exchange header only when the normalized allowlist names it. The list
is empty by default. This is a breaking change: user headers are not forwarded
unless allowlisted.

Denial runs before the allowlist check. Reserved JMS names, the `jms` and
`camel` framework namespaces, and credential substrings are denied even when
listed. QoS headers are set after extraction. `Content-Type` stays a dedicated
field, and inbound mapping is unchanged. Use valid JMS property names, such as
`traceId` and `orderId`. The producer does not sanitize property names, and
Artemis can drop a hyphenated name (`rc-ooczk`).

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

## Broker-native precedence

The bridge cap is terminal only when it fires before a provider or broker
native redelivery or expiry limit. Where the native limit fires first,
it takes precedence, and the bridge does not claim its terminal policy ran.
Redelivery is not promised to the source queue. Observed with default ActiveMQ
Classic configuration: the bridge cap fired first, the source queue was empty,
and `ActiveMQ.DLQ` `EnqueueCount` was 0. Artemis destinations were not observed
(`rc-5c6yv`). Not every native-first ordering or dead-letter query is verified.

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
