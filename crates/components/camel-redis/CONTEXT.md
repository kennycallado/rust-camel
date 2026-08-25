## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

- **(c) system-broken** (consumer.rs L202, L207, L420):
  - L202 = consumer task returned error during `stop()` shutdown lifecycle.
  - L207 = join error during `stop()` shutdown lifecycle.
  - L420 = retry-exhaustion: max transient-error attempts exceeded, consumer `return Err` ends route lifecycle.
  Each site keeps `error!` with `// log-policy: system-broken`. No metric call (operator alert via error! is the signal).
  The transient-budget Err branches in `run_pubsub_consumer` and `run_queue_consumer`
  (the `is_transient_redis_error(&e)` arm) fire
  `increment_errors(route_id, "e:redis:message-transient-budget")` BEFORE the
  `error!` — the operator signal that reconnect budget exhaustion ended the
  route. This is the transient counterpart of `e:redis:message-non-transient`.

- **(b′) outside-contract** (consumer.rs L308, L404):
  - L308 = PubSub `ctx.send()` failure (channel closed). Calls `runtime.metrics().increment_errors(route_id, "b-prime:redis:pubsub-channel-closed")` BEFORE the `error!`.
  - L404 = BLPOP `ctx.send()` failure (channel closed). Calls `runtime.metrics().increment_errors(route_id, "b-prime:redis:blpop-channel-closed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.

- **(c) system-broken** (consumer.rs L443):
  - L443 = non-transient Redis error. The route terminates on this error
    (`return Err(e)`), so supervision restarts the whole Route (ADR-0007).
    Calls `runtime.metrics().increment_errors(route_id, "e:redis:message-non-transient")`
    BEFORE the `error!`. Classified system-broken because the consumer task
    ends and the route lifecycle is interrupted, not continued per-message.

- **Transient-retry `warn!` sites** (category (e) outside-contract, ADR-0012):
  the shared retry step in `retry.rs` (`transient_retry_step`, used by the
  queue/pubsub reconnect loops for resolve/connect/pop/subscribe retries),
  `pubsub.rs` (stream-end reconnect), and
  `executor.rs` (`execute_with_retry`) log at `warn!` while retrying with
  backoff. Each carries `// log-policy: outside-contract`. These are the
  in-progress retry signal; budget exhaustion surfaces as the `error!` in
  `consumer.rs` (system-broken) when the loop returns `Err`. The
  budget-exhaustion error text is built only by
  `retry.rs::retry_budget_exhausted` — the word "connection" in that message
  is what makes `is_transient_redis_error` classify it transient (ADR-0012).

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

### `effective_tls()` (`fn effective_tls`, config.rs:346)

Auto-enables TLS for non-loopback hosts when `tls=false`. Logic in `effective_tls()`:
- Returns `true` if `tls` is explicitly true, OR host is not `localhost`, `127.*`, `::1`, or `0.0.0.0`.
- Triggers a `tracing::warn!` at runtime (`build_url()`, `apply_defaults()`) reporting the auto-enable.
- `validate_tls()` returns `Config` error if the `redis` crate lacks a TLS feature (`tls-rustls-*` or `tls-native-tls`).

### `connection_timeout_secs` (`struct RedisConfig`, config.rs:253)

Default: 10 seconds (in `RedisConfig::default()`). Applied at 4 connection sites via `tokio::time::timeout`:
- **Health check** (`fn connect_and_ping`, health.rs:38): `get_multiplexed_async_connection()` wrapped in `connection_timeout`.
- **Producer** (`MultiplexedExecutor::get_conn`, executor.rs:268): lazy `get_multiplexed_async_connection()` wrapped in `connection_timeout`.
- **Consumer PubSub** (`fn run_pubsub_consumer`, consumer.rs:266): `RedisPubSubIo::new(config.connection_timeout_secs)` wraps the pub/sub connect in `connection_timeout`.
- **Consumer Queue** (`fn run_queue_consumer`, consumer.rs:342): `RedisQueueIo::new(config.connection_timeout_secs, ...)` wraps the multiplexed connect in `connection_timeout`.

### Health check outer timeout (`fn new`, health.rs:76)

Derived from `connection_timeout_secs + 5` seconds. The outer timeout at `check()` (`fn check`, health.rs:113)
must exceed the inner connection timeout so the inner fires first with a specific error message.

## Sentinel topology

### `RedisTopology` seam (`fn topology_from_config`, topology.rs:266)

Connection targets are resolved through the `RedisTopology` trait
(`resolve(ServerKind)`), never through a fixed URL. Two implementations exist:
- **`StandaloneTopology`** — returns a client for one fixed, structurally
  built connection (address, database, credentials) for both
  `ServerKind::Master` and `ServerKind::Replica`. Default for non-sentinel
  deployments.
- **`SentinelTopology`** (feature `sentinel`) — re-queries the Sentinel cluster
  for the current master on every `resolve(ServerKind::Master)` call. The master
  address is never cached, so failover is detected on the next resolution.

### URI forms and config

Sentinel is selected either by URI scheme or by a structured config block:
- `redis-sentinel://node1:26379,node2:26379/<master-name>/<db>?command=...`
  (nodes comma-separated in the authority; `rediss-sentinel://` enables TLS).
- `[components.redis.sentinel]` with `nodes` (`Vec<String>`), `master_name`
  (`String`), and optional `username`/`password` for sentinel auth.

Both are mutually exclusive with cluster config (ADR-0033). Without the
`sentinel` cargo feature, any sentinel URI or non-empty sentinel block fails
closed at startup. Sentinel node URLs are redacted in logs: credentials are
percent-encoded into the node URL only inside `SentinelTopology::new`
(`embed_sentinel_creds`), never printed.

### Bounded reconnect vs supervision (ADR-0007)

The transport reconnect loops in `queue.rs` / `pubsub.rs` / `executor.rs` are
bounded by `NetworkRetryPolicy`. On budget exhaustion the consumer returns
`Err`, which ends the consumer task and lets Route supervision restart the whole
Route (ADR-0007). Bounded transport reconnect is **not** consumer
self-supervision: the loops only retry transient transport errors within the
budget; they never restart the Route themselves.

### Best-effort PubSub delivery

PubSub delivery is best-effort. A failover or reconnect can lose in-flight
messages, and subscription replay after reconnect can redeliver messages
already seen (duplicates possible). Consumers must tolerate both.

### Test seams

- **Producer**: `RedisTopology` + `RedisCommandExecutor` (fake executor) drive
  `MultiplexedExecutor` without a live Redis.
- **Consumers / health**: `QueueIo`, `PubSubIo`, and `HealthProbe` traits
  abstract the transport so `run_queue_consumer`, `run_pubsub_consumer`, and
  `RedisHealthCheck` are testable against fake I/O.

## Scope boundary

camel-redis follows the Component, Endpoint, Consumer, and Producer contracts
defined in `crates/components/CONTEXT.md`. Three scheme components create
`RedisEndpoint` values: `RedisComponent` for `redis:`, `RedisSentinelComponent`
for `redis-sentinel:`, and `RedissSentinelComponent` for `rediss-sentinel:` —
the registry resolves route URIs by scheme, so each scheme needs its own
registration (`RedisBundle::register_all` registers all three). The three
share one endpoint-creation path, `create_redis_endpoint`. Each Endpoint
creates an outbound `RedisProducer` or a supported inbound `RedisConsumer`.

The component does not implement EIPs. EIP behavior belongs in
`camel-processor`, so ADR-0046 behavioral parity does not apply to this crate.
`RedisCommand` defines the supported Redis command surface. Operator-owned URI
configuration selects a command before exchange processing starts.

## Trust boundary

ADR-0032 defines exchange headers and bodies as untrusted. camel-redis does not
use exchange data as a Redis command name. The command comes from the
operator-owned Endpoint URI and must parse as a `RedisCommand` variant.

Keys, fields, values, channels, scores, and other dynamic values cross the
boundary as command arguments. `redis::Cmd::arg` and `AsyncCommands` encode
these values as length-prefixed Redis protocol arguments instead of
concatenating them into command text. Argument contents therefore cannot add a
second command or change the selected command. Missing required headers return
`CamelError`.

The component does not expose `EVAL`, `EVALSHA`, or script-loading commands.
This argument framing prevents command-syntax injection. It does not make the
operation or its target key semantically safe. Routes must still validate those
values when their meaning creates a resource or authorization decision under
ADR-0032.

## Dependency boundary

The `redis` crate is a direct dependency in 11 production source files:
`producer.rs`, `consumer.rs`, `health.rs`, and the eight files under
`commands/`. No project-owned adapter trait wraps it.

ADR-0020 does not govern this boundary. That ADR isolates the beta `siumai` SDK
because LLM provider APIs can change rapidly. camel-redis uses `redis` as its
protocol driver, and its command modules intentionally use the driver types
directly. Reassess this boundary if driver API churn makes changes spread beyond
the component's Redis-specific modules.
