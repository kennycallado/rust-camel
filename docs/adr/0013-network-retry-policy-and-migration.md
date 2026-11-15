# NetworkRetryPolicy and Retry Migration Boundaries

Status: Accepted

Amended: 2026-11 — transport invalidation rule and consecutive-failure budget (bd rc-2un5k, rc-n64k3); execution-outcome classification (change redisfc)

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

### Transport invalidation and the retry budget

A cached connection is only a candidate. A transport failure proves the cache wrong, so a retry must not reuse the same socket.

The executor caches the connection as `Option<(u64, MultiplexedConnection)>`. The `u64` is a generation drawn from an allocator shared by every clone. `execute_command` and the producer `check_connection` compare the generation they observed with the cached one when `invalidates_connection` classifies the failure. On a match they clear the cache. On a mismatch a fresher connection already exists, so they leave it alone. The error path never dials. It rethrows the original error unchanged. The lazy rebuild happens on the next checkout, under the single-flight gate, and it re-resolves the topology. `reconnect` follows the same rule. It clears only the generation this instance last used for a command. A stale clone then cannot clobber the fresh connection of a sibling.

The retry budget counts consecutive failures since the last proven round trip. It has no time window. A connect is never proof, because a failing master can accept the handshake and then die. Each loop shape names its own reset point:

- Streams read half: reset on `BatchDone` after a failure-free read round trip. A transient ack failure between the read and the batch end keeps the budget consumed. `reset_read_budget` compares the counter against the snapshot taken at the last good read.
- Streams recovery half: reset on a completed `XAUTOCLAIM` pass, with or without claimed entries.
- Queue consumer: reset on a pop that yields `Ok(Some)` or `Ok(None)`.
- Pub/sub consumer: reset on a stream end whose connection uptime is at least `policy.max_delay`. A quiet channel offers no other health proof.

The recovery drain-ack path is a deliberate exception. A drain ack consumes budget only through the reconnect internals. A pass that completes still resets the counter. An ack that always fails transiently therefore oscillates without exhausting the budget. Pass success remains the recovery half's proof of health. The read half differs, because it guards its reset with an explicit failed-ack check.

Redis now follows the same consecutive-failure model as the Kafka, MQTT, and JMS consumers.

## Execution-outcome classification (redisfc)

The transport classifier answers one question: is this error transient? It does not answer a second question: may this attempt be replayed? A transient error that proves the request never executed is safe to replay under the retry budget. A transient error after the request may have reached the server is not.

`RetryClass` adds the second answer. `retry_class(err, phase)` maps a failed attempt plus its `SendPhase` onto exactly one bucket:

- **not-executed** (`RetryClass::NotExecuted`): the request provably never ran. A pre-send dial or checkout failure qualifies. A whole-request `LOADING` or `BUSY` server reply also qualifies. These attempts always replay under the retry budget, even for a non-idempotent command.
- **outcome-unknown** (`RetryClass::OutcomeUnknown`): the request may have run. An I/O failure after the flush qualifies, and so does any failure whose flush status is unknown. These attempts replay only when the request is idempotent. Idempotence is decided per command by `is_idempotent_command`, for a batch only when every entry is idempotent, and for `EVAL`/`FCALL` only with the endpoint `idempotent` opt-in.
- **never-transient** (`RetryClass::NeverTransient`): a setup defect or a semantic/business reply. These attempts never replay.

`AttemptFailure` carries the `SendPhase` next to the error. The executor records `SendPhase::PreSend` when checkout or dialing fails, and `SendPhase::PostFlush` when the request was handed to the connection before the failure.

### Precedence

`retry_class` applies four rules in strict order:

1. Config and validation errors, plus a native `redis::ErrorKind::AuthenticationFailed`, return `NeverTransient`. The phase is irrelevant.
2. The server codes `LOADING` and `BUSY` return `NotExecuted`. The whole request was rejected before it ran, so the phase is irrelevant.
3. `is_transient_redis_error` returns `NotExecuted` for `SendPhase::PreSend` and `OutcomeUnknown` for `SendPhase::PostFlush`.
4. Everything else is a semantic rejection and returns `NeverTransient`. The set covers `EXECABORT`, per-command `EXEC` errors, `NOAUTH`, `WRONGPASS`, `NOPERM`, `WRONGTYPE`, `BUSYKEY`, `NOSCRIPT`, and general `ERR …` replies.

The `READONLY` reply is the deliberate exception. A write against a replica is a structural transient error, and it invalidates the connection because the topology is stale. `retry_class` keeps it in rule 3, so the phase decides the bucket. Replay stays conservative: no replay for a non-idempotent command after the flush.

`LOADING` and `BUSY` retry on the same connection. They are whole-request rejections that prove nothing about the socket, so `invalidates_connection` returns false. A redial happens only on an invalidating failure, and it consumes the retry budget.

### Invalidation is not replayability

Invalidation and replayability are two questions with two answers. The invariant `invalidates_connection(e) ⇒ is_transient_redis_error(e)` holds over error classes. It does not depend on the replay decision.

A non-idempotent command that fails after the flush with a connection-reset error shows the separation. The error is transient, so `invalidates_connection` is true and the socket is discarded. The bucket is `OutcomeUnknown`, so the command is never replayed. The caller sees the error and decides. A non-invalidating server rejection (`LOADING`, `BUSY`) is the mirror case: the connection stays, and the request replays on it.

### Script provenance

A script reply is not trustworthy evidence about execution. A Lua script can run its side effects and then return an error reply with any code, for example `redis.error_reply('BUSY ...')` or a forged `NOSCRIPT`. A post-send script error reply therefore does not prove non-execution, and the reply code alone cannot classify it.

The dispatch resolves each script reply before classification. Every script invocation travels as one atomic provenance pair: `MULTI; SCRIPT EXISTS <sha>; EVALSHA; EXEC`. The fcall mirror uses `FUNCTION LIST`, because `FUNCTION EXISTS` does not exist in Redis. The pair is atomic, so the presence observation cannot race a concurrent `SCRIPT FLUSH` or function deletion.

The pair decides the meaning of each reply:

- The target is present and the reply is `NOSCRIPT` or "function not found". The reply is forged. The script ran, so the result is a business error. The dispatch does not re-invoke the script.
- The script runtime reports a runtime terminal failure. The script ran exactly once and its outcome stands. This is also a business error.
- A transport-class failure happens after the send. The script may or may not have run. This is `OutcomeUnknown`, and it replays only under the endpoint `idempotent` opt-in.

`retry_class_script` implements this distinction. It maps a named script business error to `NeverTransient`, a pre-send failure to `NotExecuted`, and a post-send transport failure to `OutcomeUnknown`. `execute_with_retry` selects the classifier per request kind. Plain requests keep `retry_class`. A batch entry is a plain command and cannot forge a reply, so the whole-request `LOADING`/`BUSY` provenance holds for it.

### Script trust boundary

Script text is operator configuration under ADR-0032. It comes from the endpoint URI only, through the `script` or `scriptFile` parameter. No exchange header, body, or property can supply or change script text. `scriptFile` resolves against the route-file directory. Exchange data can still reach a script as an `ARGV` entry. That data is an argument, never code.

### Why no WATCH

WATCH/EXEC needs a private connection per transaction. Each `RedisProducer::new` builds its own `MultiplexedExecutor` with a fresh connection cache, so separate endpoints do not share a connection. Concurrent exchanges on clones of one producer do share one `MultiplexedConnection`, because the derived `Clone` copies the cache handle and the generation tag. WATCH state bound to a connection would therefore leak across those exchanges. Optimistic transactions are deliberately not offered. A caller that needs read-modify-write atomicity writes one script instead, and that path follows the provenance and trust rules above.

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
  - `rc-2un5k` and `rc-n64k3` amended this ADR with the transport invalidation rule and the consecutive-failure budget for the Redis executor and consumer loops.
  - The `redisfc` change amended this ADR with the execution-outcome classification, the script provenance rule, and the no-WATCH rationale.
  - Container test dead-store cleanup (pre-existing `attempt = 0` before `break`).
