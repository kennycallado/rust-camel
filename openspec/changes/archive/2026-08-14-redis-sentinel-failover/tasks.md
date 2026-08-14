# Tasks: redis-sentinel-failover

## Phase 1: Topology seam + config + executor refactor + producer integration

### camel-component-redis

#### Task 1.1: RedisTopology trait, ServerKind, StandaloneTopology, FakeTopology

**Files:**
- `crates/components/camel-redis/src/topology.rs` (new)
- `crates/components/camel-redis/src/lib.rs` (modified) — add `pub mod topology;` and re-export `RedisTopology`, `ServerKind`, `StandaloneTopology`.

**Steps:**
1. Create `topology.rs`. Define `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum ServerKind { Master, Replica }` (MVP uses `Master` only; `Replica` is a named variant excluded by scope, not a placeholder).
2. Define the object-safe async trait:
   ```rust
   #[async_trait]
   pub trait RedisTopology: Send + Sync {
       async fn resolve(&self, kind: ServerKind) -> Result<redis::Client, CamelError>;
   }
   ```
3. Implement `StandaloneTopology { url: String }` with `resolve` returning `redis::Client::open(&self.url)` mapped to `CamelError::ProcessorError` on failure. Add `StandaloneTopology::new(url: impl Into<String>) -> Self`.
4. Implement `FakeTopology` (under `#[cfg(test)]` OR gated `pub` in a test-support module so producer/consumer unit tests in the same crate can use it): holds `Vec<Result<String, CamelError>>` programmable outcomes and an `AtomicUsize` counter; `resolve` pops the next outcome — for `Ok(addr)` returns `Client::open(addr)`, for `Err(e)` returns the `Err(e)` directly — then bumps the counter. `FakeTopology::new(outcomes: Vec<Result<String, CamelError>>) -> Self`, plus a convenience `FakeTopology::addrs(addresses: Vec<String>) -> Self` (wraps each in `Ok`) and `resolve_call_count(&self) -> usize`. The programmable-Err capability is required so downstream tests can simulate "no master resolvable" without a broker.
5. Register the module in `lib.rs` and re-export the public types.

**Tests:**
- `standalone_topology_resolve_returns_fixed_client`: GIVEN `StandaloneTopology::new("redis://127.0.0.1:6379")` WHEN `resolve(Master)` is called twice THEN both return `Ok(Client)` (the `Client` is not connected until used; assert `is_ok()` and that both targets parse the same URL — verify via the client's connection info if exposed, else assert no panic and `Ok`).
- `fake_topology_returns_address_sequence`: GIVEN `FakeTopology::addrs(vec!["redis://a:6379", "redis://b:6379"])` WHEN `resolve(Master)` is called 3 times THEN calls 1 and 2 return `Ok` for a/b and `resolve_call_count() == 3` (the 3rd reuses the last address for index >= len).
- `fake_topology_returns_programmed_error`: GIVEN `FakeTopology::new(vec![Err(CamelError::ProcessorError("no master"))])` WHEN `resolve(Master)` is called THEN it returns `Err(CamelError::ProcessorError("no master"))` and `resolve_call_count() == 1` (deterministic failure injection — no broker).

**Acceptance:**
- `cargo build -p camel-component-redis` succeeds.
- `cargo test -p camel-component-redis --lib topology` passes.
- `cargo clippy -p camel-component-redis -- -D warnings` clean.
- `cargo fmt --check --all` clean.

- [x] 1.1

#### Task 1.2: SentinelTopology impl (feature-gated) wrapping redis-rs SentinelClient

**Files:**
- `crates/components/camel-redis/src/topology.rs` (modified)
- `crates/components/camel-redis/Cargo.toml` (modified) — make the existing empty `sentinel` feature enable `redis/sentinel`: `sentinel = ["redis/sentinel"]`.

**Steps:**
1. Under `#[cfg(feature = "sentinel")]`, implement `SentinelTopology`:
   ```rust
   pub struct SentinelTopology {
       client: Mutex<redis::sentinel::SentinelClient>,
   }
   ```
   Build via `SentinelTopology::new(sentinel_nodes: Vec<String>, master_name: String, sentinel_creds: Option<(String, String)>, node_conn_info: Option<SentinelNodeConnectionInfo>) -> Result<Self, CamelError>`. Sentinel credentials (`sentinel_creds`) are embedded into the sentinel node URLs via a PURE helper `fn embed_sentinel_creds(node: &str, creds: &Option<(String,String)>) -> String` (redis-rs authenticates sentinel connections via the URL `redis://user:pass@host:port`) — do NOT pass them through `node_conn_info`, which carries the **redis-node** credentials only. Call `redis::sentinel::SentinelClient::build(nodes_with_sentinel_creds, master_name, node_conn_info, SentinelServerType::Master)` (map errors to `CamelError::ProcessorError`). This keeps sentinel-vs-node credentials separate per ADR-0051. The `embed_sentinel_creds` helper is the deterministic, no-network unit-test surface for the credential-isolation scenario.
2. `SentinelTopology::new` performs deterministic INPUT validation (no DNS): empty `sentinel_nodes` OR empty `master_name` → `Err(CamelError::Config("sentinel requires nodes and master_name"))`. This is the deterministic construction-error surface (NOT a network test).
3. `resolve(ServerKind::Master)` calls `self.client.lock().await.get_async_connection()`'s client-resolution path — specifically, return a `redis::Client` for the resolved master. NOTE: redis-rs `SentinelClient` exposes `get_client()` / client resolution; if it only exposes `get_async_connection()`, then `resolve` returns the `redis::Client` that `SentinelClient` internally resolves via `get_master_addr()` — call `redis::Client::open(resolved_addr)`. The implementer MUST inspect redis-rs 1.5.0's `sentinel.rs` (`~/.cargo/registry/src/*/redis-1.5.0/src/sentinel.rs`) and pick the API that returns the resolved master address/client WITHOUT caching it across calls (re-resolve every `resolve()`). Do NOT cache the resolved URL. (Live reachability is NOT unit-tested here — it is covered by Task 4.1's integration suite.)
4. `resolve(Replica)` returns `Err(CamelError::ProcessorError("replica reads not yet supported"))` (named excluded scope, not a placeholder).

**Tests:**
- `sentinel_topology_rejects_empty_nodes`: GIVEN `SentinelTopology::new(vec![], "m".into(), None, None)` WHEN constructed THEN returns `Err(CamelError::Config(_))` (deterministic, no network).
- `sentinel_topology_rejects_empty_master_name`: GIVEN `SentinelTopology::new(vec!["redis://s:26379".into()], "".into(), None, None)` WHEN constructed THEN returns `Err(CamelError::Config(_))`.
- `embed_sentinel_creds_keeps_credentials_separate`: GIVEN `embed_sentinel_creds("redis://s-a:26379", &Some(("sentinel-user","sentinel-pass".to_string())))` WHEN compared to the `node_conn_info` built from app creds THEN the sentinel URL carries `sentinel-user:sentinel-pass` and the node `SentinelNodeConnectionInfo` carries the app creds — the two are NOT swapped (pure-fn unit test, owns the spec scenario "sentinel and node credentials are applied without crossover"). Also assert `embed_sentinel_creds(node, &None)` returns the node unchanged.
- `sentinel_topology_replica_resolve_errors`: GIVEN a constructed `SentinelTopology` (valid nodes/master_name) WHEN `resolve(Replica)` THEN returns `Err` with "replica reads not yet supported".

**Acceptance:**
- `cargo build -p camel-component-redis --features sentinel` succeeds.
- `cargo test -p camel-component-redis --features sentinel --lib topology` passes.
- `cargo clippy -p camel-component-redis --features sentinel -- -D warnings` clean.
- **Spec scenario "sentinel resolve re-resolves the master on every call":** structurally covered — `SentinelTopology` has NO cached-master field (only the `Mutex<SentinelClient>`), and `resolve()` calls into `SentinelClient`'s master-resolution each time. The re-resolution is also proven at composition by Task 1.4's `multiplexed_executor_reconnect_reresolves` and end-to-end by Task 4.1.

- [x] 1.2

#### Task 1.3: SentinelConfig + redis-sentinel URI parsing + mutual-exclusion + feature-disabled fail-closed

**Files:**
- `crates/components/camel-redis/src/sentinel_config.rs` (new) — `SentinelConfig`, `TopologyKind`, URI parsing, `validate_topology`. Split into a sibling module so the already-1875-line `config.rs` does not deepen.
- `crates/components/camel-redis/src/config.rs` (modified) — `mod sentinel_config;` re-export, wire `sentinel: SentinelConfig` field + `topology_kind` into `RedisConfig`/`RedisEndpointConfig`, delegate sentinel validation/parsing to the new module.
- `crates/components/camel-redis/src/lib.rs` (modified) — re-export `SentinelConfig`, `TopologyKind`.

**Steps:**
1. Create `sentinel_config.rs`. Define `pub struct SentinelConfig { pub nodes: Vec<String>, pub master_name: String, pub username: Option<String>, pub password: Option<String> }` (NOT feature-gated — it must always be deserializable so the config loader can RECOGNIZE sentinel config and emit the fail-closed error when the `sentinel` cargo feature is off; only the `SentinelTopology` construction in Task 1.2 is feature-gated) with `Default` (empty) and builders `with_nodes` / `with_master_name` / `with_sentinel_credentials`.
2. Define `pub enum TopologyKind { Standalone, Sentinel(SentinelConfig), #[cfg(feature="cluster")] Cluster }` (`Sentinel` variant present unconditionally so parsing/validation work without the feature; only converting it to a live `SentinelTopology` requires the feature).
3. Extend URI parsing (a free fn `parse_sentinel_uri(uri: &str) -> Result<TopologyKind, CamelError>` in `sentinel_config.rs`, called from `config.rs::from_uri`) to recognize `redis-sentinel://` and `rediss-sentinel://`: parse `redis-sentinel://s-a:26379,s-b:26379/<master-name>/<db>?command=...` into `nodes=[s-a:26379, s-b:26379]`, `master_name=<master-name>`, `db=<db>`, plus query params. `rediss-sentinel://` sets TLS on both sentinel and node connections (reuse `effective_tls`). Sentinel `username`/`password` come from `[components.redis.sentinel]`; node creds from the top-level `[components.redis]`.
4. Add `pub fn validate_topology(kind: &TopologyKind, cluster_nodes_present: bool) -> Result<(), CamelError>` returning `CamelError::Config(...)` when: (a) Sentinel configured AND `cluster_nodes` non-empty (mutual exclusion, ADR-0033); (b) sentinel configured but `master_name` empty.
5. Add feature-disabled fail-closed (NOT behind `#[cfg(feature="sentinel")]`): in `from_uri` and the config loader, if a `redis-sentinel://`/`rediss-sentinel://` URI or non-empty sentinel block is supplied AND `cfg!(not(feature = "sentinel"))`, return `Err(CamelError::Config("redis-sentinel requires the 'sentinel' cargo feature"))`. Call `validate_topology` from component/endpoint create.
6. Wire: when constructing a `SentinelTopology` (Task 1.2), pass `SentinelConfig.username`/`password` as the `sentinel_creds` parameter and the top-level Redis node creds via `SentinelNodeConnectionInfo` — this is the crossover-prevention boundary.

**Tests:**
- `redis_sentinel_uri_parses_to_sentinel_topology`: GIVEN `redis-sentinel://s-a:26379,s-b:26379/orders/0?command=GET` WHEN `RedisEndpointConfig::from_uri` THEN topology is Sentinel with nodes `[s-a:26379, s-b:26379]`, master_name `orders`, db 0, command GET.
- `rediss_sentinel_uri_enables_tls`: GIVEN `rediss-sentinel://s-a:26379/orders/0` WHEN parsed THEN `effective_tls()` is true.
- `standalone_redis_uri_unchanged`: GIVEN `redis://127.0.0.1:6379?command=GET` WHEN parsed THEN topology is Standalone and behavior matches pre-change (regression sentinel).
- `sentinel_and_cluster_together_rejected`: GIVEN a config with non-empty sentinel.nodes AND non-empty cluster_nodes (under `cluster` feature) WHEN `validate_topology` THEN `Err(Config(...))`.
- `sentinel_scheme_rejected_without_feature`: GIVEN crate built WITHOUT `sentinel` feature WHEN `RedisEndpointConfig::from_uri("redis-sentinel://s-a:26379/orders/0")` THEN `Err(CamelError::Config("redis-sentinel requires the 'sentinel' cargo feature"))` (mark `#[cfg(not(feature="sentinel"))]`).
- `sentinel_missing_master_name_rejected`: GIVEN non-empty nodes, empty master_name WHEN `validate_topology` THEN `Err(Config(...))`.

**Acceptance:**
- `cargo test -p camel-component-redis --features sentinel --lib config` passes.
- `cargo test -p camel-component-redis --lib config` (no features) passes — the fail-closed test runs.
- `cargo clippy -p camel-component-redis --all-features -- -D warnings` clean.

- [x] 1.3

#### Task 1.4: Refactor executor.rs — real MultiplexedExecutor + topology-aware reconnect

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified) — gains `dispatch_command` (moved from `producer.rs`) + `MultiplexedExecutor`.
- `crates/components/camel-redis/src/producer.rs` (modified) — `dispatch_command` removed (moved to executor).
- `crates/components/camel-redis/src/lib.rs` (modified) — re-export `MultiplexedExecutor`.

**Steps:**
1. **Move** `RedisProducer::dispatch_command` (the big `match cmd { ... }` over `RedisCommand`) verbatim from `producer.rs` into `executor.rs` as a free fn `pub async fn dispatch_command(cmd: &RedisCommand, conn: &mut redis::aio::MultiplexedConnection, exchange: &mut Exchange) -> Result<(), CamelError>`. Update `producer.rs` to no longer define it (the producer calls it through the executor now). This removes the executor→producer upward coupling (Finding from review: executor should own the full execute path).
2. Add `pub struct MultiplexedExecutor { config: RedisEndpointConfig, topology: Arc<dyn RedisTopology>, conn: Arc<Mutex<Option<redis::aio::MultiplexedConnection>>> }`. Implement `Clone` (fields are `Arc`/`Clone`). Constructor `MultiplexedExecutor::new(config: RedisEndpointConfig, topology: Arc<dyn RedisTopology>) -> Self` with `conn = None`.
3. Move `get_or_create_connection` logic into `MultiplexedExecutor::get_conn(&self) -> Result<MultiplexedConnection, CamelError>` declared `pub(crate)` (Task 1.5's `RedisProducer::check_connection` calls it from the sibling `producer.rs`): returns the cached clone if `Some`, else calls `self.topology.resolve(ServerKind::Master).await?`, builds a multiplexed connection via `client.get_multiplexed_async_connection()` wrapped in `tokio::time::timeout(Duration::from_secs(config.connection_timeout_secs), ...)`, caches it, returns a clone. Map errors to `CamelError::ProcessorError` using `config.redis_url_safe()` in messages.
4. Implement `RedisCommandExecutor` for `MultiplexedExecutor`: `execute_command(cmd, exchange)` calls `self.get_conn()`, then `dispatch_command(cmd, &mut conn, exchange)`. `reconnect(&mut self)` clears the cache (`*guard = None`) and calls `self.get_conn()` to rebuild (which re-resolves via topology).
5. The existing `execute_with_retry<E: RedisCommandExecutor>(...)` signature is **UNCHANGED** — it already calls `executor.reconnect()` before each retry. Verify its existing unit tests still pass (they use `FakeExecutor` and do not exercise topology). No behavior change for the fake path.

**Tests:**
- `multiplexed_executor_lazy_connects_via_topology`: GIVEN a `MultiplexedExecutor` with a `FakeTopology` and NO live redis WHEN `execute_command(GET)` is called THEN it attempts `topology.resolve(Master)` (assert `FakeTopology.resolve_call_count() >= 1`) and returns `Err` for the connection failure (no panic).
- `multiplexed_executor_reconnect_reresolves`: GIVEN a `MultiplexedExecutor` with a `FakeTopology` WHEN `reconnect()` is called twice THEN `resolve_call_count() == 2` (each reconnect re-resolves; no caching).
- Existing `executor.rs` tests (`test_retry_succeeds_after_transient_failures`, `test_retry_exhausted_after_max_retries`, `test_non_idempotent_command_not_retried`, `test_non_transient_error_not_retried`, `test_idempotent_command_retried_on_transient`, `test_immediate_success_no_retries`) STILL PASS unchanged (regression guard).

**Acceptance:**
- `cargo test -p camel-component-redis --lib executor` passes.
- `cargo clippy -p camel-component-redis -- -D warnings` clean.

- [x] 1.4

#### Task 1.5: Wire producer onto topology+executor seams; producer failover tests

**Files:**
- `crates/components/camel-redis/src/producer.rs` (modified)
- `crates/components/camel-redis/src/bundle.rs` (modified) — construct the topology from the endpoint config when creating the producer.

**Steps:**
1. Change `RedisProducer` to hold `executor: MultiplexedExecutor` (replacing the raw `conn: Arc<Mutex<Option<...>>>` field) plus a copy of `config`. `RedisProducer::new(config)` constructs the topology from `config` (`StandaloneTopology` or, under the `sentinel` feature, `SentinelTopology`) and wraps it in `Arc<dyn RedisTopology>`, then builds the `MultiplexedExecutor`.
2. `RedisProducer::call()` (the `Service<Exchange>` impl): resolve command, apply defaults, then call `execute_with_retry(&mut self.executor, &cmd, &mut exchange, is_idempotent_command(&cmd), &self.config.reconnect)`. Remove the now-redundant inline retry loop in `call()` (lines ~301-396 of the current producer) — `execute_with_retry` + `MultiplexedExecutor::reconnect` subsume it.
3. `RedisProducer::check_connection()` (health pre-check): use `self.executor.get_conn()` then `PING`. (The dedicated sentinel-aware `RedisHealthCheck` is Task 3.1; here just keep `check_connection` working through the executor.)
4. `get_or_create_connection` free fn is removed from producer.rs (its logic moved into `MultiplexedExecutor::get_conn` in Task 1.4). Remove the old free fn.
5. In `bundle.rs` (or wherever the producer is constructed from the endpoint), pass the endpoint config through unchanged — topology construction happens in `RedisProducer::new`.

**Tests:**
- `producer_reconnects_before_retry_on_transient`: GIVEN a `FakeExecutor` whose first `GET` fails transiently and second succeeds WHEN `execute_with_retry(executor, GET, idempotent=true, policy)` runs THEN it returns `Ok` and `FakeExecutor.reconnect_count() == 1` and `FakeExecutor.call_count() == 2` (reconnect-before-retry, proven generically — no topology). The master re-resolution is proven separately by Task 1.4's `multiplexed_executor_reconnect_reresolves`; the two compose correctly in production. (This test intentionally uses `FakeExecutor`, NOT `MultiplexedExecutor`+`FakeTopology` — a pure `FakeExecutor` never touches the topology, so `resolve_call_count` must NOT be asserted here.)
- `producer_does_not_replay_non_idempotent_command`: GIVEN a `FakeExecutor` whose first `INCR` fails transiently WHEN `execute_with_retry` with `is_idempotent=false` runs THEN it returns `Err`, `FakeExecutor.call_count() == 1`, and `FakeExecutor.reconnect_count() == 0` (no reconnect, no re-resolution).
- `producer_standalone_unchanged`: GIVEN a `RedisProducer` with `StandaloneTopology` from `redis://127.0.0.1:6379?command=PING` (existing unit tests in producer.rs that don't need live redis — keep them passing; the `test_producer_new`, `test_producer_clone_shares_connection`, `test_resolve_command_*` tests must still pass after the refactor).

**Acceptance:**
- `cargo test -p camel-component-redis --lib producer` passes.
- `cargo clippy -p camel-component-redis -- -D warnings` clean.
- `cargo build -p camel-component-redis --features sentinel` succeeds.
- `cargo xtask lint-log-levels` passes (Task 1.4 reconnect sites classified per ADR-0012; do NOT defer to Phase 3).
- `cargo xtask lint-secrets` passes (no raw passwords logged at the new reconnect sites per ADR-0051).

- [x] 1.5

## Phase 2: Consumer paths — queue + pubsub resubscription replay

### camel-component-redis

#### Task 2.1: Queue consumer — injectable QueueIo seam + topology re-resolution

**Files:**
- `crates/components/camel-redis/src/consumer.rs` (modified) — `QueueIo` trait, real impl, refactor of `run_queue_consumer`.

**Steps:**
1. `RedisConsumer` gains a `topology: Arc<dyn RedisTopology>` field, constructed in `RedisConsumer::new` from the endpoint config (same construction as the producer in Task 1.5).
2. Define an injectable I/O seam so the failover loop is testable WITHOUT a broker:
   ```rust
   #[async_trait]
   pub(crate) trait QueueIo: Send {
       async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError>;
       async fn blpop(&mut self, key: &str, timeout_secs: u64) -> Result<Option<(String, String)>, CamelError>;
   }
   ```
   Real impl `RedisQueueIo { conn: Option<redis::aio::MultiplexedConnection>, timeout }`: `connect` calls `client.get_multiplexed_async_connection()` wrapped in `connection_timeout`; `blpop` runs the BLPOP command and maps transient errors.
3. Refactor `run_queue_consumer` to be generic over `Box<dyn QueueIo>` (the spawned task owns it). The loop: `client = topology.resolve(Master).await?; io.connect(&client).await?;` then loop `blpop`. On a transient error, instead of retrying on the SAME connection, re-resolve (`topology.resolve(Master)`), `io.connect(&client)` again, then retry — bounded by `config.reconnect`. On `max_attempts` exhaustion return `Err` (supervision fires). On success reset the attempt counter.
4. Keep the cancel-token shutdown check and `ctx.send()` failure metrics (`b-prime:redis:blpop-channel-closed`, `e:redis:message-non-transient`) — ADR-0012 log-policy sites MUST stay.
5. Add a `FakeQueueIo` test double: programmable `connect_outcomes: Vec<Result<(), CamelError>>` and `blpop_outcomes: Vec<Result<...>>`, popped in order; records `connect_count`.

**Tests:**
- `queue_recovers_after_connection_loss`: GIVEN a `FakeQueueIo` whose first `connect` returns `Err(transient)` and second `connect` returns `Ok`, and `blpop` returns `Some(("k","v"))` WHEN `run_queue_consumer` runs with a `FakeTopology.addrs(["redis://a","redis://b"])` THEN the loop recovers: it returns after emitting one Exchange, `FakeTopology.resolve_call_count() >= 2` (re-resolved after the loss), and `FakeQueueIo.connect_count() == 2`. Fully deterministic — no broker.
- `queue_returns_err_when_failover_budget_exhausted`: GIVEN a `FakeQueueIo` whose `connect` always returns `Err(transient)` AND a `FakeTopology` always returning a dummy client WHEN `run_queue_consumer` runs with `max_attempts=3` THEN the task returns `Err` and `FakeQueueIo.connect_count() == 3` (proves Err → supervision, distinct path).

**Acceptance:**
- `cargo test -p camel-component-redis --lib consumer` passes (existing consumer unit tests still pass; new deterministic failover tests added).
- `cargo clippy -p camel-component-redis -- -D warnings` clean.

- [x] 2.1

#### Task 2.2: PubSub consumer — injectable PubSubIo seam + subscription replay

**Files:**
- `crates/components/camel-redis/src/consumer.rs` (modified) — `PubSubIo` trait, real impl, refactor of `run_pubsub_consumer`; `subscribe_all` helper.

**Steps:**
1. Define an injectable I/O seam:
   ```rust
   #[async_trait]
   pub(crate) trait PubSubIo: Send {
       async fn connect(&mut self, client: &redis::Client) -> Result<(), CamelError>;
       async fn subscribe(&mut self, ch: &str) -> Result<(), CamelError>;
       async fn psubscribe(&mut self, pat: &str) -> Result<(), CamelError>;
       async fn next_msg(&mut self) -> Option<Msg>;  // None => stream ended
   }
   ```
   Real impl `RedisPubSubIo { pubsub: Option<redis::aio::PubSub>, timeout }`: `connect` calls `client.get_async_pubsub()` wrapped in `connection_timeout`; `subscribe`/`psubscribe` map redis errors to `CamelError`; `next_msg` polls `on_message()`.
2. Extract `async fn subscribe_all<P: PubSubIo>(io: &mut P, channels: &[String], patterns: &[String]) -> Result<(), CamelError>` looping `subscribe` then `psubscribe`. Increment a passed-in `subscribe_call_count` (or assert via the fake's recording).
3. Refactor `run_pubsub_consumer` to be generic over `Box<dyn PubSubIo>`: on start, resolve master, `connect`, `subscribe_all`. Message loop reads `next_msg()`; on `None` (stream end) OR a transient error, re-resolve, reconnect, `subscribe_all` (replay ALL channels/patterns), resume — bounded by `config.reconnect`; on exhaustion return `Err`.
4. Keep the shutdown cancel-token check and the `b-prime:redis:pubsub-channel-closed` metric on `ctx.send()` failure (ADR-0012).
5. Add a `// log-policy` note and a best-effort delivery comment pointing to the spec's "best-effort delivery: loss and duplicates possible" wording.
6. Add a `FakePubSubIo` test double: programmable `connect_outcomes`, `next_msg` queue (with a sentinel for "stream ended after N"), records `subscribe`/`psubscribe` args.

**Tests:**
- `subscribe_all_replays_every_channel_and_pattern`: GIVEN a `FakePubSubIo` WHEN `subscribe_all(&mut io, &["a","b"], &["ev*"])` is called THEN `io.subscribed_channels == ["a","b"]` and `io.subscribed_patterns == ["ev*"]` (deterministic, no broker).
- `pubsub_resubscribes_after_stream_end`: GIVEN a `FakePubSubIo` whose `next_msg` returns one message then `None` (stream end), channels `[a,b]`, pattern `ev*`, AND a `FakeTopology.addrs(["redis://a","redis://b"])` WHEN `run_pubsub_consumer` runs THEN after the stream-end the consumer reconnects against b (`resolve_call_count() >= 2`) and re-invokes `subscribe_all` (`io.subscribed_channels` length reflects a second replay pass). End-to-end message delivery after a real failover is covered by Task 4.1.
- `pubsub_returns_err_on_budget_exhaustion`: GIVEN a `FakePubSubIo` whose `connect` always returns `Err(transient)` WHEN `run_pubsub_consumer` runs with `max_attempts=3` THEN the task returns `Err` (distinct from the queue path).

**Acceptance:**
- `cargo test -p camel-component-redis --lib consumer` passes (existing consumer unit tests still pass; new failover tests added).
- `cargo clippy -p camel-component-redis -- -D warnings` clean.
- `cargo xtask lint-log-levels` passes (Phase 2 transient-retry `warn!` sites classified per ADR-0012; do NOT defer to Phase 3).
- `cargo xtask lint-secrets` passes (redacted URLs at new consumer reconnect sites per ADR-0051).

- [x] 2.2

#### Task 2.3: Remove dead code + verify standalone consumer regression

**Files:**
- `crates/components/camel-redis/src/consumer.rs` (modified, if needed)

**Steps:**
1. Audit `run_queue_consumer` and `run_pubsub_consumer` for any leftover `redis::Client::open(config.redis_url())` calls — replace any stragglers with `topology.resolve`. (Both should already be done in 2.1/2.2; this task is the verification sweep.)
2. Confirm the standalone (`redis://`) consumer path constructs a `StandaloneTopology`. **Behavior-change note (intentional, per spec):** the PubSub stream-end path is **changed** from today's `break`→`Ok(())` (graceful EOF) to bounded-reconnect-then-`Err` — because a standalone PubSub that silently ends and returns Ok would mask a dead master. With `StandaloneTopology`, the reconnect loop re-resolves the same fixed URL (equivalent connection target) but the loop now returns `Err` on budget exhaustion so Route supervision fires. The **Queue** path's transient-budget→`Err` behavior is **unchanged** (consumer.rs:425 already returns `Err` on exhaustion today). Do NOT preserve the old `break`→`Ok` on PubSub stream-end.
3. Run the full consumer test suite to confirm no regression.

**Tests:**
- `standalone_consumer_uses_standalone_topology`: GIVEN a `redis://` endpoint WHEN the consumer is constructed THEN its topology is `StandaloneTopology` (assert via a debug accessor or construction log).

**Acceptance:**
- `grep -rn "Client::open(config.redis_url())" crates/components/camel-redis/src/` returns ZERO hits in consumer.rs (the producer's old free fn is already removed in 1.4).
- `cargo test -p camel-component-redis --lib` passes.
- `cargo clippy -p camel-component-redis -- -D warnings` clean.

- [x] 2.3

## Phase 3: Health, logs, docs

### camel-component-redis

#### Task 3.1: Sentinel-aware RedisHealthCheck

**Files:**
- `crates/components/camel-redis/src/health.rs` (modified)

**Steps:**
1. `RedisHealthCheck` gains a `topology: Arc<dyn RedisTopology>` field (constructed from the endpoint config, same as producer/consumer) AND an injectable probe seam `Arc<dyn HealthProbe>` so the PING is testable WITHOUT a broker:
   ```rust
   #[async_trait]
   pub(crate) trait HealthProbe: Send + Sync {
       async fn connect_and_ping(&self, client: &redis::Client, timeout: Duration) -> Result<(), CamelError>;
   }
   ```
   Real impl `RedisHealthProbe`: opens a multiplexed connection (wrapped in `connection_timeout`) and runs `PING`, mapping failure to `Err`.
2. In `check()`: call `topology.resolve(ServerKind::Master).await?`; on resolve `Err` return `Unhealthy` (do NOT fall back to the stale configured node). Then `probe.connect_and_ping(&client, inner_timeout).await`; map `Ok`→`Healthy`, `Err`→`Unhealthy`.
3. Keep the outer timeout (`connection_timeout_secs + 5`, per CONTEXT.md health.rs:91) exceeding the inner connection timeout.

**Tests:**
- `health_probes_current_master_not_stale`: GIVEN a `RedisHealthCheck` with a `FakeTopology.addrs(["redis://b:6379"])` (single address — the current master is b; no a in play) AND a `FakeHealthProbe` (records the client it was probed with; returns `Ok`) WHEN `check()` runs THEN `check()` returns `Healthy`, `resolve_call_count() == 1`, and `FakeHealthProbe` was probed with b's client. Fully deterministic — no broker.
- `health_unhealthy_when_no_master_resolvable`: GIVEN a `RedisHealthCheck` with a `FakeTopology::new(vec![Err(CamelError::ProcessorError("no master"))])` WHEN `check()` runs THEN the result is `Unhealthy` and the probe was NOT invoked.
- `health_unhealthy_when_ping_fails`: GIVEN a resolve that returns a client AND a `FakeHealthProbe` returning `Err` WHEN `check()` runs THEN the result is `Unhealthy`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib health` passes.
- `cargo clippy -p camel-component-redis -- -D warnings` clean.

- [x] 3.1

#### Task 3.2: Redacted logs + ADR-0012 log-policy annotations + CONTEXT.md

**Files:**
- `crates/components/camel-redis/src/topology.rs` (modified)
- `crates/components/camel-redis/src/consumer.rs` (modified)
- `crates/components/camel-redis/src/producer.rs` (modified, if logs added)
- `crates/components/camel-redis/CONTEXT.md` (modified)

**Steps:**
1. Ensure every log site that prints a Redis address uses `redis_url_safe()` (never the raw URL with password) per ADR-0051. In `SentinelTopology::resolve`, `run_queue_consumer`, `run_pubsub_consumer`, and `MultiplexedExecutor::get_conn`/`reconnect`, log `info!`/`warn!` with the safe endpoint and a `master_changed`/`reconnecting` message — redacted.
2. Add `// log-policy: <category>` annotations for any new `error!`/`warn!` sites classifying them per ADR-0012 (likely `(c) system-broken` for budget-exhaustion, `(b′)/(e) outside-contract` for channel-closed carryovers).
3. Update `CONTEXT.md`: add a "Sentinel topology" section documenting the `RedisTopology` seam, `redis-sentinel:` URI, the bounded-reconnect-vs-supervision boundary (ADR-0007), and best-effort PubSub delivery. Update the "Crash health ownership" note if the boundary wording needs refinement.
4. Run `cargo xtask lint-log-levels` and `cargo xtask lint-context-citations` — fix any findings (the CONTEXT.md citations must resolve).

**Tests:**
- No new unit tests; verification is via the lints below.

**Acceptance:**
- `cargo xtask lint-log-levels` passes (no new violations; new sites classified).
- `cargo xtask lint-secrets` passes (no raw passwords logged).
- `cargo xtask lint-context-citations` passes.
- `cargo clippy -p camel-component-redis -- -D warnings` clean.

- [x] 3.2

#### Task 3.3: User docs + redis-sentinel example

**Files:**
- `docs/src/components/redis.md` (modified)
- `examples/redis-example/` (modified) — add a sentinel example route (or a `redis-sentinel` example) behind the `sentinel` feature; keep it `#[ignore]`-free and testcontainers-driven if it's a test, OR a plain runnable example.

**Steps:**
1. In `docs/src/components/redis.md`, add a "Sentinel / failover" section: the `redis-sentinel://` and `rediss-sentinel://` URI forms, the `[components.redis.sentinel]` TOML block with separate sentinel vs node credentials, the bounded-reconnect behavior, the best-effort PubSub caveat, and a YAML + Rust example.
2. Add an example route (or extend `examples/redis-example`) showing a `redis-sentinel://` producer and a SUBSCRIBE consumer. If it requires a live broker, wire it as a testcontainers-driven `camel-test` integration test (Task 4.1 owns the test; here just the doc + a runnable example snippet).
3. Ensure the doc prose is in English (project language policy).

**Tests:**
- No new unit tests.

**Acceptance:**
- `cargo xtask lint-context-citations` passes (doc references resolve).
- `cargo build -p redis-example --features camel-component-redis/sentinel` (or the example's feature) succeeds.
- `cargo fmt --check --all` clean.

- [x] 3.3

## Phase 4: Sentinel failover integration suite (camel-test + testcontainers)

### camel-test

#### Task 4.1: Self-provisioned sentinel-failover integration test

**Files:**
- `crates/camel-test/tests/redis_sentinel_test.rs` (new)
- `crates/camel-test/Cargo.toml` (modified) — ensure the `integration-tests` feature gates this test; confirm `testcontainers`/`testcontainers-modules` are present (they are, per ADR-0054).

**Steps:**
1. Create `redis_sentinel_test.rs` behind `#[cfg(feature = "integration-tests")]`. Self-provision via testcontainers: one Redis master container, one Redis replica configured to replicate, and a Sentinel container (use the `redis` testcontainers image with a custom sentinel config, or a dedicated sentinel image). Configure the sentinel to monitor the master with a known `master_name` and quorum 1.
2. Test: register a `RedisComponent` with a `redis-sentinel://<sentinel-host>:26379/<master_name>/0?command=GET` endpoint. Assert a producer write (`SET`) + read (`GET`) round-trips.
3. Trigger failover: issue `SENTINEL FAILOVER <master_name>` against the sentinel (via a redis client), OR promote the replica / demote the master. Wait with a HARD bounded deadline (e.g. 60s) polling the producer until a subsequent `SET`/`GET` succeeds against the newly elected master.
4. Repeat for a SUBSCRIBE consumer: subscribe, trigger failover, assert the consumer resubscribes and delivers a message published after the failover.
5. The test MUST NOT use `#[ignore]` (ADR-0054). It runs in CI's `full-tests-linux` job behind `--features integration-tests`. Add a `mod` doc comment stating this.

**Tests:**
- `producer_recovers_after_sentinel_failover` (the test itself): GIVEN master+replica+sentinel via testcontainers WHEN `SENTINEL FAILOVER` is issued THEN a producer `SET`/`GET` succeeds against the new master within the deadline.
- `queue_consumer_recovers_after_sentinel_failover`: GIVEN a `BRPOP` consumer running against the master via testcontainers WHEN `SENTINEL FAILOVER` is issued THEN an item pushed to the list after the failover is delivered as an Exchange within the deadline (covers the Queue path the spec's integration requirement names).
- `pubsub_consumer_resubscribes_after_sentinel_failover`: GIVEN a SUBSCRIBE consumer WHEN failover occurs THEN a message published post-failover is received.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_sentinel_test` passes with a Docker daemon (CI owns this; the conductor does NOT run it locally).
- `cargo xtask lint-ignore` passes — no `#[ignore]`/`requires live` annotations introduced.
- The test is skipped by default (`cargo test -p camel-test` without the feature) — confirm it does not compile-fail without the feature.

- [x] 4.1
