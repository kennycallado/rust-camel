# Tasks: redis-repositories

## Phase 1: seam and service-crate foundation

### camel-component-redis

#### Task 1.1: widen the component connection seam

**Files:**
- `crates/components/camel-redis/src/topology.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/lib.rs` (modified)

**Steps:**
1. In `topology.rs`, change `pub(crate) fn topology_from_config(` (line ~302) to `pub fn topology_from_config(`. Signature and body unchanged.
2. In `lib.rs`, add `pub use topology::topology_from_config;` next to the existing `pub use topology::{RedisTopology, ServerKind, StandaloneTopology};` (line ~58).
3. In `executor.rs`, change `pub(crate) async fn get_conn(&self)` (line ~268) to `pub async fn get_conn(&self)`. Signature unchanged.
4. In `executor.rs`, add an inherent method on `MultiplexedExecutor`:
   `pub async fn refresh(&self) -> Result<MultiplexedConnection, CamelError>` — body mirrors the trait `reconnect` (executor.rs:324-332): take the cached-connection state lock (`Arc<Mutex<Option<MultiplexedConnection>>>`, executor.rs:251), clear the cached connection, then `self.get_conn().await`. No `&mut self`; no trait change.
5. Run the existing camel-redis test suite; no behavior change is expected anywhere.

**Tests:** (all in `crates/components/camel-redis`, existing test modules)
- `refresh_triggers_reresolution_and_failure_is_not_cached`: a `MultiplexedExecutor` over `FakeTopology` (counting resolves) pointed at an unreachable endpoint → `get_conn()` → `Err`; a second `get_conn()` re-resolves (`resolve_count == 2` — failures are never cached, executor.rs:268-305); `refresh()` → `Err` and increments the resolve count by exactly one more. The healthy-path refresh behavior (cached handle cleared, reconnect succeeds) is covered live by the retry-once integration tests in task 2.7.
  - command: `cargo test -p camel-component-redis refresh`
  - expected: pass after step 4; before it, the method does not compile.
- `get_conn_and_topology_from_config_are_pub`: compile-only usage test (or doc example) constructing `camel_component_redis::topology_from_config(&config)` from outside the crate (`tests/` target) → compiles.
  - command: `cargo test -p camel-component-registry 2>/dev/null; cargo build -p camel-component-redis` plus the compile check in `crates/services/camel-redis-repo` (task 1.2).
  - expected: pass.

**Tests:**
- `component_sentinel_gate_regression_without_service_crate`: the component built WITHOUT its sentinel feature still rejects `redis-sentinel://` URIs and sentinel config blocks at startup (the existing `cfg(not(sentinel))` fail-closed tests, config.rs:2136-2194) — this graph does not link `camel-redis-repo`, owning the delta scenario "component feature gate unchanged for graphs without the service crate".
  - command: `cargo test -p camel-component-redis --no-default-features sentinel`
  - expected: pass (pre-existing behavior, must not regress).

**Acceptance:**
- `cargo test -p camel-component-redis` exits 0.
- `cargo test -p camel-component-redis --no-default-features sentinel` exits 0.
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0.
- `rg -n 'pub\(crate\)' crates/components/camel-redis/src/topology.rs | rg topology_from_config` returns nothing.

- [x] 1.1

### camel-redis-repo

#### Task 1.2: scaffold the publishable service crate

**Files:**
- `crates/services/camel-redis-repo/Cargo.toml` (new)
- `crates/services/camel-redis-repo/src/lib.rs` (new)
- `crates/services/camel-redis-repo/README.md` (new)
- `Cargo.toml` (modified — root workspace)

**Steps:**
1. Root `Cargo.toml`: add to `[workspace.dependencies]` an entry `camel-redis-repo = { path = "crates/services/camel-redis-repo", version = "0.33.0" }` following the exact-version + path pattern of the neighboring service crates.
2. Crate manifest: inherit `version`, `edition`, `license`, `repository`, `homepage` via `.workspace = true`; set `description = "Redis-backed CacheRepository and IdempotentRepository for rust-camel"`, `documentation = "https://docs.rs/camel-redis-repo"`, `readme = "README.md"`, `keywords = ["camel", "redis", "cache", "idempotent", "integration"]`, `categories = ["database", "asynchronous"]`; add `[lints] workspace = true`.
3. Dependencies: `camel-api` (workspace), `camel-component-redis` (workspace, `features = ["sentinel"]` — the controlling feature branch, topology.rs:293-329), `redis` (workspace, `features = ["tokio-comp", "aio", "sentinel"]`), `tokio`, `async-trait`, `serde_json`, `tracing` (workspace). Add a `tls` feature forwarding to `camel-component-redis/tls` and the redis tls features, mirroring the component's own `tls` feature. No `sentinel` feature — it is always on.
4. Add a test-only forwarding feature to the manifest: `cluster = ["camel-component-redis/cluster"]` — labeled in a comment as existing solely so the cluster-rejection unit test (task 1.5) can construct a cluster-shaped topology; never enabled by default, never used by the repositories.
5. `src/lib.rs`: ONLY crate docs and `pub use camel_component_redis::{RedisEndpointConfig, SentinelConfig};` (downstream crates never need the component as a direct dependency). Do NOT declare modules whose files do not exist yet — each owning task (1.3: `keyspace`, `error`; 1.4: `executor`; 1.5: `connection`; 2.1: `cache_repo`; 3.1: `idempotent_repo`) adds its own `mod`/`pub use` line when it creates the file. Empty stub modules are forbidden.
6. `README.md`: short usage sketch — `[default.cache_repo] backend = "redis"`, `url = "redis://127.0.0.1:6379"`, and the sentinel variant (`sentinel_nodes` + `master_name`).
7. Verify `crates/services/*` workspace globs already include the crate: `cargo metadata` lists `camel-redis-repo`.

**Tests:**
- `crate_compiles_with_sentinel_unified`: after scaffolding, `cargo build -p camel-redis-repo` compiles with the sentinel feature of the component enabled (feature unification documented in design decision 4).
  - command: `cargo build -p camel-redis-repo && cargo tree -p camel-redis-repo -e features | rg 'camel-component-redis feature "sentinel"'`
  - expected: build succeeds; feature list shows sentinel active.

**Acceptance:**
- `cargo build -p camel-redis-repo` exits 0.
- `cargo xtask lint-publish-cycles` exits 0.
- `cargo xtask lint-component-deps` exits 0 (crate is under `services/`, not scanned as a component, and must stay unscanned).

- [x] 1.2

#### Task 1.3: keyspace helpers and error mapping

**Files:**
- `crates/services/camel-redis-repo/src/keyspace.rs` (new)
- `crates/services/camel-redis-repo/src/error.rs` (new)
- `crates/services/camel-redis-repo/src/lib.rs` (modified)

**Steps:**
1. `keyspace.rs`: `pub fn namespaced(prefix: &str, repo: &str, key: &str) -> String` returning `format!("{prefix}:{repo}:{key}")`.
2. `keyspace.rs`: `pub fn validate_namespace_token(kind: &str, value: &str) -> Result<(), CamelError>` — rejects empty values and any character outside `[A-Za-z0-9:_-]` with `Err(CamelError::Config(format!("{kind} '{value}': must be non-empty and use only [A-Za-z0-9:_-] (glob metacharacters are forbidden)")))`. `kind` is a human label used in the message ("repository name" or "key_prefix").
3. `error.rs`: `pub fn to_camel_error(err: redis::RedisError) -> CamelError` mapping every transport/command failure to `CamelError::Io(err.to_string())`. Transience classification for retry decisions uses the component's `camel_component_redis::is_transient_redis_error` (already `pub`, config.rs:903) — re-export it from `lib.rs` for internal use. No new error variant.
4. Export both modules from `lib.rs`.

**Tests:**
- `namespaced_builds_hierarchical_key`: `namespaced("camel:cache", "default", "k")` → `"camel:cache:default:k"`.
  - command: `cargo test -p camel-redis-repo namespaced`
  - expected: pass once step 1 lands.
- `validate_rejects_glob_metacharacters`: `validate_namespace_token("repository name", "my*cache")` → `Err(CamelError::Config(_))` containing the allowed charset. (Owns delta scenario "cache repository name with glob metacharacters rejected at construction" at the helper level; the constructor test lands in 2.1/3.1.)
- `validate_rejects_empty_token`: `validate_namespace_token("key_prefix", "")` → `Err(_)`.
- `validate_accepts_colon_tokens`: `validate_namespace_token("key_prefix", "camel:cache")` → `Ok(())`.
- `to_camel_error_maps_to_io`: a `redis::RedisError` (e.g. `RedisError::from((ErrorKind::IoError, "connection reset"))`) → `CamelError::Io(_)` whose `classify()` is `"io"`.
  - command: `cargo test -p camel-redis-repo`

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.
- `cargo clippy -p camel-redis-repo -- -D warnings` exits 0.

- [x] 1.3

#### Task 1.4: RepoCommandExecutor seam with production and fake implementations

**Files:**
- `crates/services/camel-redis-repo/src/executor.rs` (new)
- `crates/services/camel-redis-repo/src/lib.rs` (modified)

**Steps:**
1. Define the crate-internal seam:
   `#[async_trait] pub(crate) trait RepoCommandExecutor: Send + Sync { async fn execute(&self, cmd: redis::Cmd) -> Result<redis::Value, CamelError>; async fn refresh(&self) -> Result<(), CamelError>; }`
2. `pub(crate) struct MultiplexedRepoExecutor { inner: camel_component_redis::MultiplexedExecutor }` with `pub(crate) fn new(inner: MultiplexedExecutor) -> Self`. `execute` clones the connection via `inner.get_conn().await` — mapping ANY resulting error (including the component's `CamelError::ProcessorError` connection failures) to `CamelError::Io(e.to_string())` — and runs `cmd.query_async(&mut conn).await` mapping errors with `error::to_camel_error`. `refresh` calls `inner.refresh().await` and maps the error to `CamelError::Io(..)` the same way. Rationale: the repository contract requires transport failures to be `Io` (classify() == "io", Contract C1 path); the component's `ProcessorError` mapping must not leak through the repo seam.
3. `pub(crate) struct FakeRepoExecutor` (in-crate; unit tests live in the same crate): holds `Mutex<Vec<redis::Cmd>>` recorded commands, `AtomicUsize` execute/refresh counters, and a `Mutex<VecDeque<Result<redis::Value, CamelError>>>` scripted results queue (empty queue → `Ok(redis::Value::Nil)`). Methods: `push_result(result: Result<redis::Value, CamelError>)`, `commands() -> Vec<redis::Cmd>`, `execute_count() -> usize`, `refresh_count() -> usize`.
4. `FakeStaticTopology` (in `executor.rs`, `pub(crate)`, unit tests live in-crate): implements `camel_component_redis::RedisTopology`, returning a scripted `redis::Client` per `resolve()` call and counting resolves with an `Arc<AtomicUsize>`. The component's own `FakeTopology` is `#[cfg(test)]`-gated (its lib.rs re-export is dead for downstream non-test builds) and cannot be imported — this is why the local fake exists.
5. `FakeRedisServer` (`pub(crate)`, same module) with an eager-bind API: `FakeRedisServer::start() -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)>` — binds a `tokio::net::TcpListener` on `127.0.0.1:0` BEFORE returning (spawn the accept loop on the returned handle's task), accepts connections, and for each one loops: consume ONE complete RESP request frame (a RESP array `*N\r\n` followed by N `$<len>\r\n<bytes>\r\n` elements — read until all N elements are consumed; N may be 0 or 1 for inline commands), then write exactly one `+PONG\r\n` response. This gives the healthy-connection path a deterministic in-process endpoint (no server, no Docker) and one response per command frame.
5. Export the module from `lib.rs` (items stay `pub(crate)`).

**Tests:**
- `fake_records_commands_and_counts`: drive `FakeRepoExecutor::execute` with a `redis::Cmd::new().arg("PING")` twice → `commands().len() == 2`, `execute_count() == 2`, results come from the scripted queue.
- `fake_counts_refreshes`: call `refresh()` once → `refresh_count() == 1`.
- `multiplexed_execute_maps_transport_error_to_io`: build a real `MultiplexedExecutor` over `FakeStaticTopology` scripted to resolve an unreachable endpoint (`redis://127.0.0.1:1/0`), wrap in `MultiplexedRepoExecutor::new(..)` → `execute(Cmd::new().arg("PING"))` returns `Err(CamelError::Io(_))` — including the `get_conn` failure path, which the component maps to `ProcessorError` and this wrapper must remap to `Io` (connection refused is deterministic; no server needed). The full live path is exercised in task 2.7's integration suite.
  - command: `cargo test -p camel-redis-repo`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.
- `cargo clippy -p camel-redis-repo --all-targets -- -D warnings` exits 0.

- [x] 1.4

#### Task 1.5: connection module — resolve once, reject cluster

**Files:**
- `crates/services/camel-redis-repo/src/connection.rs` (new)
- `crates/services/camel-redis-repo/src/lib.rs` (modified)

**Steps:**
1. Two constructors:
   - `pub(crate) async fn connect_executor(endpoint: &RedisEndpointConfig) -> Result<MultiplexedRepoExecutor, CamelError>` — builds the topology via `topology_from_config`, then delegates to the injected variant.
   - `pub(crate) async fn connect_executor_with_topology(endpoint: &RedisEndpointConfig, topology: Arc<dyn RedisTopology>) -> Result<MultiplexedRepoExecutor, CamelError>` — the test seam (FakeStaticTopology / FakeRedisServer plug in here); both share the body:
   - Build the component topology input from the endpoint. Read `crates/components/camel-redis/src/topology.rs` (`topology_from_config`, line ~302) and `src/config.rs` first to use the exact existing input type (`RedisConfig`/`RedisEndpointConfig` per its signature) — do not invent a parallel endpoint type.
   - Reject cluster up front, BEFORE calling `topology_from_config`: if the endpoint's topology selector is `TopologyKind::Cluster` (or any cluster node list is present when the forwarding feature exposes it), return `Err(CamelError::Config("cluster topology is not supported for repository backends"))` — this exercises the repository-specific rejection path, not the component's own handling. (Owns delta scenario "cluster topology rejected for repositories" at the connection layer.)
   - Construct `MultiplexedExecutor::new(..)`, then perform an EAGER `get_conn().await` (mapped to `Io` through the wrapper) so construction fails fast on an unreachable topology — lazy connect would re-resolve on every failed call and would not give resolve-once semantics at construction. Do NOT wrap the resolve in `tokio::task::spawn_blocking` here: `RedisTopology::resolve` is `async fn` and the component already offloads the blocking sentinel resolve internally (topology.rs:270-275).
   - Wrap in `MultiplexedRepoExecutor`. This function runs once per repository construction; later re-resolution happens only through `refresh()` after a connection error (redis-failover delta, resolution seam).

**Tests:**
- `connect_executor_eager_connect_fails_fast_once`: `FakeStaticTopology` counting resolves, scripted to an unreachable address, via `connect_executor_with_topology` → `Err(CamelError::Io(_))` with the topology resolved exactly once at construction (eager fail-fast; failures never cached).
- `connect_executor_resolves_once_on_healthy_connection`: `FakeRedisServer` spawned; `FakeStaticTopology` counting resolves, scripted to a `redis::Client` for the stub address; `connect_executor_with_topology` → `Ok`; then TWO `execute(Cmd::new().arg("PING"))` calls through the returned executor → both `Ok`, and `resolve_count() == 1` (healthy connection cached; no per-operation resolve). (Owns delta scenario "master resolved once, not per operation" with a real healthy connection.)
- `connect_executor_rejects_cluster` (runs under the test-only `cluster` feature — `TopologyKind::Cluster` is `cfg(feature = "cluster")` in the component): build a `RedisEndpointConfig` with `topology_kind = TopologyKind::Cluster` (the endpoint type carries the selector; no cluster node fields needed), call `connect_executor` → `Err(CamelError::Config(_))` mentioning "cluster", and the rejection fires before any topology resolution. (Owns delta scenario "cluster topology rejected for repositories".)
  - command for this test: `cargo test -p camel-redis-repo --features cluster connect_executor_rejects_cluster`
- `connect_executor_maps_transport_failure_to_io`: unreachable standalone endpoint (`redis://127.0.0.1:1/0`) → `Err(CamelError::Io(_))` (Contract C1: never `Ok`).
  - command: `cargo test -p camel-redis-repo`
  - expected: pass. (The unreachable-endpoint tests need no server; connection refused is deterministic.)

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.
- `cargo clippy -p camel-redis-repo -p camel-component-redis -- -D warnings` exits 0.
- `cargo xtask lint-publish-cycles` exits 0.

- [x] 1.5

## Phase 2: RedisCacheRepository

### camel-redis-repo

#### Task 2.1: RedisCacheRepository struct and constructors

**Files:**
- `crates/services/camel-redis-repo/src/cache_repo.rs` (new)
- `crates/services/camel-redis-repo/src/lib.rs` (modified)

**Steps:**
1. `pub struct RedisCacheRepository { name: String, key_prefix: String, stale_retention: Duration, clock: ClockFn, executor: Arc<dyn RepoCommandExecutor>, hits: AtomicU64, misses: AtomicU64 }` with `pub type ClockFn = Arc<dyn Fn() -> std::time::SystemTime + Send + Sync>;` and `pub fn default_clock() -> ClockFn` (returns `Arc::new(SystemTime::now)`), both in `cache_repo.rs`.
2. `pub async fn connect(name: &str, endpoint: &RedisEndpointConfig, key_prefix: &str, stale_retention: Duration) -> Result<Self, CamelError>` — validates name/prefix (below), calls `connection::connect_executor(endpoint).await`, uses `default_clock()`. The configured `key_prefix` MUST reach the repository through this parameter (a validated-but-dropped prefix is the silent-meaningless-config trap).
3. `pub(crate) fn with_executor(name: &str, key_prefix: &str, stale_retention: Duration, clock: ClockFn, executor: Arc<dyn RepoCommandExecutor>) -> Result<Self, CamelError>` — sync; validates `validate_namespace_token("repository name", name)?` and `validate_namespace_token("key_prefix", key_prefix)?` before anything else; no network.
4. Manual `impl std::fmt::Debug for RedisCacheRepository` printing only `name`, `key_prefix`, `stale_retention` (the trait requires `Debug`; `ClockFn`/`Arc<dyn RepoCommandExecutor>` are not `Debug`, so `derive` cannot work; never print the executor or clock).
5. Re-export `RedisCacheRepository` from `lib.rs`.

**Tests:**
- `cache_constructor_rejects_glob_name`: `RedisCacheRepository::with_executor("my*cache", "camel:cache", Duration::from_secs(1), default_clock(), Arc::new(FakeRepoExecutor::new()))` → `Err(CamelError::Config(_))` naming "repository name". (Owns delta scenario "cache repository name with glob metacharacters rejected at construction".)
- `cache_constructor_rejects_empty_prefix`: `with_executor("default", "", ..)` → `Err(_)`.
  - command: `cargo test -p camel-redis-repo cache_constructor`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.

- [x] 2.1

#### Task 2.2: storage primitives as inherent methods (no trait impl yet)

**Files:**
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified)

**Steps:**
1. Do NOT open an `impl CacheRepository` block in this task — an incomplete trait impl does not compile. Implement the storage primitives as INHERENT `pub(crate)` methods on `RedisCacheRepository`; the complete trait impl (all seven methods, compiling as one unit) lands in task 2.4.
2. `pub(crate) async fn set_entry(&self, key: &str, value: CacheEntry, ttl: Option<Duration>) -> Result<(), CamelError>` (the eventual trait method is BY VALUE — cache.rs:83-91): clone into a local `entry`, set `entry.expires_at = ttl.and_then(|d| self.clock().checked_add(d))` BEFORE serialization, mirroring `camel-core/src/cache/redb.rs:423` (on `checked_add` overflow → `None` expiry). Serialize `serde_json::to_vec(&entry)`. Build ONE `redis::Cmd`: `SET {namespaced} {blob}`; when `entry.expires_at` is `Some(t)`, compute the EXAT seconds as `t.checked_add(self.stale_retention).and_then(|t2| t2.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs())` — `Some(secs)` → attach `SetExpiry::EXAT(secs)` via `redis::SetOptions` (`redis = "1.6.0"`, crate root); `None` (overflow anywhere) → plain SET with no expiry, never fail the write.
3. `pub(crate) async fn get_entry(&self, key: &str) -> Result<Option<CacheEntry>, CamelError>`: `GET` via one `redis::Cmd`; `Value::Nil` → count miss, `Ok(None)`; payload (`Value::BulkString(bytes)`) → `serde_json::from_slice` → if in-band `expires_at` present and `< clock()` → count miss, return `Ok(None)`; else hit, `Ok(Some(entry))`. Transport error → `Err(CamelError::Io(_))` — never a silent miss (Contract C1). (Retry wiring lands in 2.3.)
4. `pub(crate) async fn peek_stale_entry(&self, key: &str) -> Result<Option<CacheEntry>, CamelError>`: same fetch, ignore in-band expiry; transport error → `Err(Io)`.
5. `pub(crate) async fn invalidate_key(&self, key: &str) -> Result<(), CamelError>`: one `UNLINK {namespaced}` cmd; `Value::Nil` or any int → `Ok(())`.

**Tests:** (all with `FakeRepoExecutor` + fixed clock)
- `set_get_roundtrip`: `set_entry("k", entry.clone(), None)` then `get_entry("k")` with the fake echoing back `serde_json::to_vec(&entry)` for GET → `Ok(Some(entry))` equal to stored. (Owns delta scenario "set and get round-trip through Redis".)
- `exat_applied_only_with_expiry`: fixed clock `now`, `stale_retention = 10s`, `set_entry("k", e.clone(), Some(Duration::from_secs(30)))` then `set_entry("k", e, None)` → recorded commands: first contains EXAT unix-secs `now+40`, second has no EXAT; each is ONE command. (Owns delta scenario "EXAT applied only when the entry carries expires_at".)
- `in_band_expiry_enforced_on_get_peek_stale_still_reads`: entry with `expires_at = now-1s` (still within retention), fake GET returns it → `get_entry` → `Ok(None)`; `peek_stale_entry` → `Ok(Some(entry))`. (Owns delta scenario "in-band expiry enforced on get, peek_stale still reads".)
- `get_err_on_transient_never_silent_miss`: fake GET returns `Err(CamelError::Io("connection refused"))` (retry also errors) → `get_entry` → `Err(CamelError::Io(_))`, NOT `Ok(None)`. (Owns delta scenario "get surfaces backend failure as Err, never as silent miss".)
- `invalidate_unlinks_namespaced_key`: `invalidate_key("k")` → one recorded `UNLINK camel:cache:default:k`.
  - command: `cargo test -p camel-redis-repo`
  - expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.
- `cargo clippy -p camel-redis-repo --all-targets -- -D warnings` exits 0.

- [x] 2.2

#### Task 2.3: retry-once policy for retry-safe cache operations

**Files:**
- `crates/services/camel-redis-repo/src/executor.rs` (modified — the free helper)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified)

**Steps:**
1. Add the shared FREE helper in `executor.rs`: `pub(crate) async fn execute_retry_safe(ex: &Arc<dyn RepoCommandExecutor>, cmd: redis::Cmd) -> Result<redis::Value, CamelError>` — first `ex.execute(cmd.clone())`; on `Err` where the error is transient (classify by calling the component's `is_transient_redis_error(&err)` — its existing signature takes a `&CamelError`, config.rs:903), call `ex.refresh()`, re-issue the SAME cmd ONCE; second failure → return the `Err`. Non-transient → `Err` immediately. (Introduced HERE, in 2.3 — not later; the idempotent repo in 3.1 reuses it.)
2. Route `set_entry`, `get_entry`, `peek_stale_entry`, `invalidate_key` through `execute_retry_safe(&self.executor, cmd)` (all retry-safe: last-writer-wins SET, idempotent GET/UNLINK — design retry table).

**Tests:**
- `set_retries_once_after_transient`: fake queue `[Err(CamelError::Io("connection reset by peer")), Ok(Value::SimpleString("OK"))]` → `set_entry` returns `Ok(())`, `execute_count() == 2`, `refresh_count() == 1`, both recorded commands identical. (Owns delta scenario "set retries once after a lost response (last-writer-wins)".)
- `get_retries_once_then_succeeds`: fake `[Err(CamelError::Io("connection reset by peer")), Ok(Value::BulkString(blob))]` → `get_entry` → `Ok(Some(entry))`, counts 2/1.
- `no_retry_on_second_failure`: fake `[Err(CamelError::Io("connection reset by peer")), Err(CamelError::Io("connection refused"))]` → `get_entry` → `Err(Io)`, `execute_count() == 2`, `refresh_count() == 1` (no third attempt).
- `non_transient_no_retry`: fake `[Err(CamelError::Config("non-transient"))]` → `Err` immediately, `execute_count() == 1`, `refresh_count() == 0`.
  - command: `cargo test -p camel-redis-repo`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.

- [x] 2.3

#### Task 2.4: scan engine, complete CacheRepository trait impl

**Files:**
- `crates/services/camel-redis-repo/src/executor.rs` (modified — scan_unlink_pattern lives here)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified)

**Steps:**
1. Shared free helper in `executor.rs`: `pub(crate) async fn scan_unlink_pattern(ex: &Arc<dyn RepoCommandExecutor>, pattern: &str) -> Result<u64, CamelError>`: loop building `SCAN {cursor} MATCH {pattern} COUNT 100` as a `redis::Cmd`; parse each reply with `redis::from_redis_value::<(u64, Vec<String>)>(&value)` (the typed tuple conversion — do NOT hand-match `redis::Value` variants, which changed across redis 1.x: `Value::Array`/`Value::BulkString`, not `Value::Bulk`); batch `UNLINK k1 k2` (one cmd per ≤100 keys), sum removed counts. Route every command through `execute_retry_safe` (introduced in 2.3); on retry the failed batch re-issues.
2. Private `pub(crate) async fn stats_snapshot(&self) -> CacheStats`: `CacheStats { hits, misses, entries: 0, evictions: 0, .. }` from the `AtomicU64` counters incremented in `get_entry`/`peek_stale_entry`.
3. NOW write the complete, compiling `#[async_trait] impl CacheRepository for RedisCacheRepository` delegating to the inherent methods (all seven — this is the first task whose diff contains the impl block):
   - `set` → `set_entry` (by-value signature, cache.rs:83-91); `get` → `get_entry`; `peek_stale` → `peek_stale_entry`; `invalidate` → `invalidate_key`; `clear` → `scan_unlink_pattern(&self.executor, "{key_prefix}:{name}:*")` then `Ok(())` — NEVER `FLUSHDB`/`FLUSHALL`.
   - `invalidate_prefix(prefix)` → FIRST `validate_namespace_token("invalidate_prefix", prefix)?` (glob metacharacters → `Err(CamelError::Config(_))` BEFORE any SCAN — ADR-0032: the prefix is a simple-language expression resolved from exchange data), then `scan_unlink_pattern(&self.executor, "{key_prefix}:{name}:{prefix}*")` returning the removed count. This overrides the trait default, which fails closed.
   - `async fn stats(&self) -> CacheStats` (match the exact async trait signature in `crates/camel-api/src/cache.rs`) → `stats_snapshot()`.

**Tests:**
- `clear_scoped_to_repository_prefix`: the fake SCAN reply contains ONLY the matching key (real Redis applies MATCH server-side): scripted via `redis::to_redis_value(&(0u64, vec!["camel:cache:default:a".to_string()]))`. After `clear()`: the recorded SCAN command's MATCH argument is exactly `camel:cache:default:*`; the recorded UNLINK commands contain ONLY `camel:cache:default:a`; no recorded command argument equals `FLUSHDB`/`FLUSHALL`; the string `camel:idem:default:b` appears in NO recorded command. (Owns delta scenario "clear deletes only the cache repository prefix".)
- `invalidate_prefix_purges_one_namespace_and_guards_step_prefix`: fake SCAN reply contains only the matching key (`redis::to_redis_value(&(0u64, vec!["camel:cache:default:ns:a".to_string()]))`) → `invalidate_prefix("ns:")` returns `1`, the recorded SCAN MATCH argument is exactly `camel:cache:default:ns:*`, UNLINK contains only `camel:cache:default:ns:a`, and the string `camel:cache:default:other:b` appears in no recorded command; separately `invalidate_prefix("ns*")` → `Err(CamelError::Config(_))` with `execute_count()` unchanged (no SCAN issued). (Owns delta scenario "invalidate_prefix purges one logical namespace and guards the step prefix".)
- `stats_one_hit_one_miss`: `get("k")` hit, `get("x")` miss (Nil) → `stats().await` → `hits == 1, misses == 1, entries == 0, evictions == 0`. (Owns delta scenario "stats reports one hit and one miss with zero entries and evictions".)
  - command: `cargo test -p camel-redis-repo`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0 (trait impl complete — the crate compiles with the full `CacheRepository` impl).
- `rg -n 'FLUSH(DB|ALL)' crates/services/camel-redis-repo/src/` returns nothing.

- [x] 2.4

### camel-config

#### Task 2.5: CacheRepoConfig redis fields, validation matrix, redacting Debug

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Extend `CacheRepoConfig` (struct at ~line 549, `#[serde(deny_unknown_fields)]`): add `url: Option<String>`, `sentinel_nodes: Option<Vec<String>>`, `master_name: Option<String>`, `sentinel_username: Option<String>`, `sentinel_password: Option<String>`, `key_prefix: Option<String>` (serde default → `"camel:cache"` applied at use site). Keep ALL existing fields (`backend`, `max_capacity`, `path`, `stale_retention`, `max_entries`, `cache_size`, `sweep_interval`) untouched.
2. Accept `backend = "redis"` everywhere the backend string is matched (validation + `context_ext` in 2.6).
3. Add validation branch for `backend == "redis"` in `validate()` implementing the delta matrix, each error naming `cache_repo.<field>` and the violated rule: neither url nor sentinel_nodes; both; empty `sentinel_nodes` list or any empty/whitespace-only entry; `master_name` empty/missing while sentinel_nodes set; `master_name`/`sentinel_username`/`sentinel_password` set without sentinel_nodes; url scheme not `redis://`/`rediss://` (parse with the `url`-style check the component uses in its own config validation — reuse the existing pattern); (defensive only) `CacheRepoConfig` carries no cluster fields — cluster rejection is owned by the connection layer (task 1.5); do not add an untestable config-level branch; `key_prefix` failing `camel_redis_repo`'s charset rule (call `camel_redis_repo`'s public validator or inline the same `[A-Za-z0-9:_-]` rule — prefer importing the service crate's validator so the rule lives once; add `camel-redis-repo` to `crates/camel-config/Cargo.toml` workspace deps in this task).
4. Redb-only fields (`path`, `cache_size`, `sweep_interval`, `max_entries`) are NOT read by the redis branch and are not required when `backend = "redis"` — add a test pinning this.
5. Hand-write `impl fmt::Debug for CacheRepoConfig` redacting: URL userinfo replaced by the literal `***` while scheme, host, port, path, and query stay verbatim (e.g. `redis://user:secret@h:6379/0` renders as `redis://***@h:6379/0`); `sentinel_password`/`sentinel_username` rendered as `Some("***")`. Keep all other fields in the output.
6. The cross-repository prefix-collision rule (cache vs idempotent on the same endpoint/db) is DEFERRED to task 3.3 (needs the generalized idempotent config); do not implement it here.

**Tests:** (in `crates/camel-config` existing config test module)
- `cache_redis_no_topology_rejected`, `cache_redis_url_and_sentinel_mutually_exclusive`, `cache_redis_empty_sentinel_entry_rejected`, `cache_redis_empty_master_name_rejected`, `cache_redis_orphan_master_name_rejected`, `cache_redis_orphan_sentinel_password_rejected`, `cache_redis_invalid_url_scheme_rejected`, `cache_redis_glob_prefix_rejected`, `cache_redis_redb_fields_not_required`: each constructs a `CamelConfig` with the offending/valid `cache_repo` → asserts the exact validation verdict; the last one asserts a minimal redis config (backend + url only) passes validation even without `path`/`cache_size`. (Owns the nine redis validation scenarios of the eip-cache MODIFIED requirement; the malformed `stale_retention` and prefix-collision scenarios are owned by main's existing tests and task 3.3 respectively.)
- `cache_debug_redacts_credentials`: `format!("{:?}", cfg)` with `url = "redis://user:secret@h:6379"`, `sentinel_password = "hunter2"` → output contains neither `secret` nor `hunter2`. (Owns delta scenario "credentials redacted from Debug output".)
  - command: `cargo test -p camel-config`
  - expected: fail before, pass after.

**Acceptance:**
- `cargo test -p camel-config` exits 0 (new tests AND all pre-existing memory/redb cache tests — the redb `cache_size` validation must keep passing untouched).
- `cargo xtask lint-secrets` exits 0.

- [x] 2.5

#### Task 2.6: context_ext registers the redis cache repository

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)
- `crates/camel-config/Cargo.toml` (modified — camel-redis-repo dep if not added in 2.5)

**Steps:**
1. In the cache_repo construction path (`build_persistent_cache_repo`, context_ext.rs:110-162, registration at context_ext.rs:305-320), add a `"redis"` arm: build a `camel_redis_repo::RedisEndpointConfig` (re-exported from the service crate) from the config fields (`url`, or `sentinel_nodes` + `master_name` + sentinel credentials, mapping to the component endpoint types exactly as `crates/components/camel-redis/src/config.rs` parses them), compute `stale_retention` (parse humantime; malformed → `Err(CamelError::Config("cache_repo.stale_retention: invalid duration '<raw value>'"))` — never a silent default), `key_prefix` default `"camel:cache"`, then `RedisCacheRepository::connect("redis", &endpoint, &key_prefix, stale_retention).await` and `ctx.register_cache_repository("redis", Arc::new(repo))`.
2. Registration errors map to `Err(CamelError::Config("cache_repo: <failure description>"))`, mirroring the redb arm's error style.

**Tests:**
- `redis_cache_arm_unreachable_url_fails_build_with_named_error`: `CamelConfig` with `cache_repo.backend = "redis"`, `url = "redis://127.0.0.1:1/0"` → building the context returns `Err(CamelError::Io/Config(_))` whose message names `cache_repo` (eager connect fails fast through the wiring; deterministic, no server). Successful live registration is owned by task 2.7's `cache_registration_via_config_live`.
- `redis_endpoint_helper_maps_url_and_sentinel`: unit-test `pub(crate) fn redis_endpoint_from_cache_repo(&CacheRepoConfig) -> Result<RedisEndpointConfig, CamelError>` directly — url-only config → standalone endpoint; sentinel config (nodes + master + credentials) → sentinel endpoint carrying all three; url + sentinel both → `Err` (mirrors validation). (Wiring-level ownership of the sentinel selection scenario; live sentinel registration is task 3.5.)
- Existing redb/memory arms are regression-guarded by running the full pre-existing `camel-config` suite in Acceptance (`redb registered when backend = redb`, `memory max_capacity supplied via config`, and the cache_size/sweep family must all stay green).
  - command: `cargo test -p camel-config`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-config` exits 0.
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 2.6

### camel-test

#### Task 2.7: integration suite — cache section, CI target

**Files:**
- `crates/camel-test/tests/redis_repositories_test.rs` (new)
- `crates/camel-test/Cargo.toml` (modified)
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. `crates/camel-test/Cargo.toml`: add `camel-redis-repo = { workspace = true, optional = true }` and add it to the `integration-tests` feature list (leaf direction only — the service crate never depends on camel-test).
2. `redis_repositories_test.rs` (no `#[ignore]` anywhere — ADR-0054): follow the `redis_sentinel_test.rs` container pattern. Cache section tests, each provisioning its own Redis container:
   - `cache_roundtrip_and_ttl`: build context from Camel.toml string with `backend = "redis"`; `cache_repository("redis")` resolves; `set("k", entry, Some(60s))` → `get` returns it; raw `TTL` on `camel:cache:redis:k` is in `(60, 100]` seconds (EXAT = now+60+retention with retention 30s → TTL ≈ 90).
   - `cache_in_band_expiry_and_peek_stale_live`: `set` with 1ms ttl, sleep 20ms, retention 60s → `get` → `Ok(None)`, `peek_stale` → `Ok(Some(_))`.
   - `cache_clear_scoped_live`: pre-insert a foreign key `camel:idem:x:y` via a raw client; repo `set` two keys; `clear()` → repo keys gone, foreign key still `EXISTS`.
   - `cache_invalidate_prefix_live`: keys `ns:a`, `other:b` → `invalidate_prefix("ns:")` returns 1, `other:b` survives.
   - `cache_registration_via_config_live`: full `CamelConfig` build with the container URL → `cache_repository("redis")` is `Some`, `name() == "redis"`, `"memory"` still resolvable. (Owns the live variants of delta scenarios "redis registered when backend = redis" and "sentinel-selected redis backend registered" — the sentinel variant lands in task 3.5.)
3. `.github/workflows/ci.yml`: next to the existing `--test redis_test` line (~line 104), add `cargo test -p camel-test --features integration-tests --test redis_repositories_test`.
4. Verify the suite runs WITHOUT `#[ignore]`: `rg -n '#\[ignore' crates/camel-test/tests/redis_repositories_test.rs` → no hits. (Owns redis-failover delta scenario "integration suite runs in CI without ignore markers".)

**Tests:**
- The five tests above ARE the executable spec.
  - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test`
  - expected: fail before implementation (crate missing), pass after; requires Docker in the environment, CI owns it otherwise.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_repositories_test` exits 0 locally (Docker available).
- CI workflow contains the new test line.
- `rg -n 'ignore' crates/camel-test/tests/redis_repositories_test.rs` returns no attribute hits.

- [x] 2.7

## Phase 3: RedisIdempotentRepository and documentation

### camel-redis-repo

#### Task 3.1: RedisIdempotentRepository — struct, constructors, core ops

**Files:**
- `crates/services/camel-redis-repo/src/idempotent_repo.rs` (new)
- `crates/services/camel-redis-repo/src/lib.rs` (modified)

**Steps:**
1. `pub struct RedisIdempotentRepository { name: String, key_prefix: String, executor: Arc<dyn RepoCommandExecutor> }`.
2. `pub async fn connect(name: &str, endpoint: &RedisEndpointConfig, key_prefix: &str) -> Result<Self, CamelError>` — validates tokens, `connection::connect_executor`; the configured prefix MUST reach the repository through this parameter.
3. `pub(crate) fn with_executor(name: &str, key_prefix: &str, executor: Arc<dyn RepoCommandExecutor>) -> Result<Self, CamelError>` — same validation as 2.1 step 3. Manual `impl std::fmt::Debug` printing only `name` and `key_prefix` (trait requires `Debug`; `Arc<dyn RepoCommandExecutor>` is not `Debug`).
4. `impl IdempotentRepository`:
   - `async fn add(&self, key: &str) -> Result<bool, CamelError>`: ONE `SET {namespaced} 1 NX` cmd through the executor (NOT through `execute_retry_safe` — outcome-bearing). Response applied → `Ok(true)`; `Nil`/not-applied → `Ok(false)`. Transport/transient error → `Err(CamelError::Io(_))` immediately, then call `executor.refresh()` (best-effort, ignore its error) so the NEXT call uses a healthy connection. NEVER re-issue the SET NX.
   - `async fn contains(&self, key)`: `EXISTS` via the free `executor::execute_retry_safe(&self.executor, cmd)` introduced in task 2.3 (no refactor needed — it is already a shared free function).
   - `async fn remove(&self, key)`: `UNLINK` retry-safe.
   - `async fn clear(&self)`: delegate to the shared `executor::scan_unlink_pattern(&self.executor, "{key_prefix}:{name}:*")` from task 2.4 (already a free function — no re-extraction needed).
5. Re-export from `lib.rs`.

**Tests:**
- `add_is_atomic_insert_if_absent`: fake returns `Ok(Value::SimpleString("OK"))` then `Ok(Value::Nil)` → first `add("msg-1")` → `Ok(true)`, second → `Ok(false)`. (Owns delta scenario "add is atomic insert-if-absent".)
- `contains_remove_roundtrip`: fake EXISTS 1 → `contains` `Ok(true)`; UNLINK → `remove` `Ok(())`; EXISTS 0 → `Ok(false)`. (Owns delta scenario "contains and remove round-trip".)
- `clear_scoped_to_idempotent_prefix`: fake SCAN reply contains only the matching key (hand-built RESP shape `Array[BulkString("0"), Array[BulkString(key)]]` — via the shared `test_support::scan_reply` helper; redis 1.6.0 removed the old `to_redis_value` free function this prescription originally used) → recorded SCAN MATCH argument is exactly `camel:idem:default:*`; UNLINK contains only `camel:idem:default:a`; the string `camel:cache:default:b` appears in no recorded command; no FLUSH commands. (Owns delta scenario "clear deletes only the idempotent repository prefix".)
- `idempotent_constructor_rejects_glob_name`: `with_executor("my*idem", ..)` → `Err(CamelError::Config(_))`. (Owns delta scenario "idempotent repository name with glob metacharacters rejected at construction".)
  - command: `cargo test -p camel-redis-repo`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0 (including all cache tests after the helper refactor).

- [x] 3.1

#### Task 3.2: Contract C1 — no re-issue of SET NX after lost response

**Files:**
- `crates/services/camel-redis-repo/src/idempotent_repo.rs` (modified)

**Steps:**
1. Pin the add policy with tests (the implementation landed in 3.1; this task adds the C1-focused tests and any guard comments). If any test forces an implementation correction, keep the invariant: exactly one SET NX per `add` call, transient → `Err(Io)` + refresh-for-later, never `Ok(true)`/`Ok(false)` on unknown outcome.
2. Document the invariant in a doc comment on `add` (lost-response reasoning from design.md §Idempotent semantics).

**Tests:**
- `transient_failure_returns_err_not_ok`: fake `[Err(CamelError::Io("connection timed out during failover"))]` → `add("k")` → `Err(CamelError::Io(_))`, NOT `Ok(true)`, NOT `Ok(false)`. (Owns delta scenario "transient failure during failover surfaces as Err".)
- `add_never_reissues_set_nx_after_lost_response`: fake `[Err(CamelError::Io("connection reset by peer"))]` then (second queue entry would succeed, proving no re-issue) → after `add("k")`: `execute_count() == 1` (exactly one SET NX issued), `refresh_count() == 1`; a subsequent `add("k2")` with fake `[Ok(OK)]` succeeds on the refreshed executor. (Owns delta scenario "add does not retry SET NX after a lost response".)
- `contains_retries_once_retry_safe`: fake `[Err(CamelError::Io("connection reset by peer")), Ok(Value::Int(1))]` → `contains` → `Ok(true)`, counts 2/1 (asymmetry vs add documented).
  - command: `cargo test -p camel-redis-repo`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0.

- [x] 3.2

### camel-config

#### Task 3.3: IdempotentRepoConfig generalization and cross-repo prefix-collision rule

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add `pub struct IdempotentRepoConfig` with `#[serde(deny_unknown_fields)]`, fields: `backend: String` with `#[serde(default = "default_idempotent_backend")]` (`fn default_idempotent_backend() -> String { "redb".into() }`), plus `path: Option<String>`, `durability: Option<String>` (redb), and `url`/`sentinel_nodes`/`master_name`/`sentinel_username`/`sentinel_password`/`key_prefix: Option<String>` (redis, default `"camel:idem"` at use site).
2. Change `CamelConfig::idempotent_repo` from `Option<RedbIdempotentConfig>` to `Option<IdempotentRepoConfig>` (config.rs:32, :145, :172). Keep `RedbIdempotentConfig` as a type if referenced elsewhere, or inline its fields — check usages (the idempotent registration block, context_ext.rs:288-297, reads `icfg.path`/durability) and adapt them to read through `backend == "redb"` (registration branch for redis lands in 3.4).
3. Validation: `backend` must be `"redb"` or `"redis"` (else error naming `idempotent_repo.backend`); redb arm keeps the existing rules verbatim (empty path rejected, durability enum); redis arm runs the SAME matrix as task 2.5 but naming `idempotent_repo.<field>` (share the matrix via a `pub(crate)` helper taking a field-name prefix, so cache and idempotent branches cannot drift).
4. Cross-repo collision rule (from 2.5 step 6): when `cache_repo` and `idempotent_repo` are both `backend = "redis"` and their effective endpoints match (same url + db, or same sentinel nodes+master+db) and effective prefixes are identical → validation error stating the prefixes must be distinct.
5. Hand-written redacting `Debug` for `IdempotentRepoConfig` (same rules as 2.5 step 5).

**Tests:**
- `existing_redb_toml_parses_unchanged`: a pre-change TOML snippet `[default.idempotent_repo] path = "x.redb"` + `durability = "eventual"` parses with `backend == "redb"`. (Owns delta scenario "existing redb TOML parses unchanged".)
- `idempotent_redb_empty_path_still_rejected`, `idempotent_redb_durability_default_immediate` (regression, owns MODIFIED redb scenarios).
- `idempotent_redis_validation_mirrors_matrix`: sentinel_nodes without master_name → error naming `idempotent_repo.master_name`; plus url+sentinel both → mutual-exclusion error naming `idempotent_repo`. (Owns delta scenario "idempotent redis validation mirrors the cache matrix".)
- `idempotent_debug_redacts_credentials`: `format!("{:?}")` omits url userinfo secret and sentinel password. (Owns delta scenario "idempotent credentials redacted from Debug output".)
- `shared_database_prefix_collision_rejected`: both redis on `redis://h:6379`, both prefixes `camel:shared` → error "must be distinct"; distinct prefixes on the same endpoint → Ok. (Owns delta scenario "shared-database prefix collision rejected".)
  - command: `cargo test -p camel-config`
  - expected: fail before, pass after.

**Acceptance:**
- `cargo test -p camel-config` exits 0 (all pre-existing tests included).
- `cargo xtask lint-secrets` exits 0.

- [x] 3.3

#### Task 3.4: context_ext registers the redis idempotent repository

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)

**Steps:**
1. In the idempotent_repo registration block (context_ext.rs:288-297), branch on `backend`: `"redb"` keeps the existing `RedbIdempotentRepository` path (reading path/durability through the generalized struct); `"redis"` builds the endpoint via a sibling helper `pub(crate) fn redis_endpoint_from_idempotent_repo(&IdempotentRepoConfig) -> Result<RedisEndpointConfig, CamelError>` (same mapping rules as 2.6's `redis_endpoint_from_cache_repo` — extract the shared field-mapping into one inner function both helpers call, so cache and idempotent mapping cannot drift), `RedisIdempotentRepository::connect("redis", &endpoint, &key_prefix).await`, `ctx.register_idempotent_repository("redis", Arc::new(repo))`.
2. Unset / `backend = "redb"` keeps memory default behavior identical.

**Tests:**
- `redis_idempotent_registered_when_configured` (live-lite): unreachable url → context build returns `Err` naming `idempotent_repo` (wiring reached); registration shape asserted live in 3.5. (Owns delta scenario "redis registered when configured" at wiring level.)
- `redb_idempotent_arm_regression`: existing redb config still registers `"redb"` and `"memory"` remains default (existing tests must stay green — run them, do not rewrite).
  - command: `cargo test -p camel-config`
  - expected: pass.

**Acceptance:**
- `cargo test -p camel-config` exits 0.

- [x] 3.4

### camel-test

#### Task 3.5: integration suite — idempotent and sentinel sections

**Files:**
- `crates/camel-test/tests/redis_repositories_test.rs` (modified)

**Steps:**
1. Idempotent section (own containers): `idempotent_add_contains_remove_live` (add → Ok(true), re-add → Ok(false), contains → true, remove → contains false); `idempotent_clear_scoped_live` (cache key survives idempotent clear); `idempotent_registration_via_config_live` (`[default.idempotent_repo] backend = "redis"` → `idempotent_repository("redis")` resolves, `"memory"` default intact).
2. Sentinel section: provision the sentinel topology exactly as `redis_sentinel_test.rs` does (master + replica + sentinels via testcontainers); `cache_sentinel_selected_by_config_live`: `cache_repo` with `sentinel_nodes` + `master_name` → context build succeeds, `cache_repository("redis")` resolves, a `set`/`get` round-trip works against the master the sentinels resolved. (Owns delta scenario "sentinel-selected redis backend registered" live variant and the redis-failover "sentinel selected by config fields, no URI scheme" live variant.)
3. Keep the no-`#[ignore]` invariant.

**Tests:**
- The four tests above are the executable spec.
  - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test`
  - expected: pass (Docker required; CI owns otherwise).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_repositories_test` exits 0.
- No `#[ignore]` attributes in the file.

- [x] 3.5

### docs

#### Task 3.6: ADR-0063 with amendments to ADR-0023 and ADR-0056

**Files:**
- `docs/adr/0063-redis-repository-service.md` (new)
- `docs/adr/0023-idempotent-repository-trait.md` (modified)
- `docs/adr/0056-cache-repository-port.md` (modified)
- `docs/adr/0028-claimcheck-repository-trait.md` (modified)

**Steps:**
1. Write ADR-0063 "Redis repository service" in the project ADR template (read 0056 as the style reference): Status accepted, date 2026-08-22; header metadata `Amends: ADR-0023, ADR-0056`; `Cross-references: ADR-0028, ADR-0032, ADR-0033, ADR-0051, ADR-0054`. Decisions recorded: service-crate placement (crates/services/camel-redis-repo, camel-auth structural analogy; reads/writes run during Exchange processing, registration at build time), both-repos-one-change, connection seam (get_conn/refresh/topology_from_config widening + redis::Cmd via RepoCommandExecutor), per-repository executor (no registry), sentinel always-compiled (component feature unification consequence; component gate unchanged for graphs without the service crate), explicit-config selection (no auto-detect, ADR-0033/0032), SET NX no-retry C1 rule, single SET…EXAT atomic write, SCAN+UNLINK clear + invalidate_prefix with charset guards, error mapping (Io/Config), cluster rejected, no idempotent TTL, no-lifecycle justification (eager connect at construction, no consumer/background task — ADR-0028 StepLifecycle rule does not apply).
2. ADR-0023: in the future-backend placement paragraph (~line 95), append one sentence: the Redis implementation ships as the `camel-redis-repo` repository service crate (`Amended by: ADR-0063`), not inside the component.
3. ADR-0056: same reciprocal note on its Redis/backend paragraph (~lines 233-238): `Amended by: ADR-0063`.
4. ADR-0028: add a cross-reference line (no amendment): Redis repository service is a separate port family; ADR-0063 records why StepLifecycle does not apply to repository connections.
5. Apply the `ste-writing` skill rules to all new prose (ASD-STE100; no slop markers).

**Tests:**
- `adr_amendments_reciprocal`: `rg -n 'Amended by: ADR-0063' docs/adr/0023*.md docs/adr/0056*.md` → 2 hits; `rg -n 'Amends: ADR-0023, ADR-0056' docs/adr/0062*.md` → 1 hit.
  - command: `rg -c 'ADR-0063' docs/adr/0063-redis-repository-service.md docs/adr/0023-idempotent-repository-trait.md docs/adr/0056-cache-repository-port.md`
  - expected: ≥1 hit each.

**Acceptance:**
- ADR file exists, numbered 0062, reciprocal notes present in 0023 and 0056.
- `cargo xtask lint-context-citations` exits 0.

- [x] 3.6

#### Task 3.7: CONTEXT-MAP and Services charter updates

**Files:**
- `CONTEXT-MAP.md` (modified)
- `crates/services/CONTEXT.md` (modified)
- `crates/camel-api/CONTEXT.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)

**Steps:**
1. `CONTEXT-MAP.md`: add the ADR-0063 index line after the 0061 entry (~line 98); add a nested `Redis Repository Service` Contexts entry under Services with a one-line description; extend the Services relationship with the Runtime repository-registration relationship (or add one line if the existing relationship text does not cover named registration); add Key Terms `Redis repository backend` and `repository service crate` citing ADR-0063; APPEND `redis` to the CacheRepository Key Term's backend list (never rewrite the existing sentence — it already documents `invalidate_prefix` and async `stats` on main).
2. `crates/services/CONTEXT.md`: add the `Repository service` definition — context-scoped, named infrastructure used during Exchange processing, not necessarily a `Lifecycle` implementation — plus `_Avoid_: service (unqualified), Component, repository adapter crate`; list `camel-redis-repo` in the family inventory.
3. `crates/camel-api/CONTEXT.md` and `crates/camel-core/CONTEXT.md`: append the Redis backend to their repository backend lists where memory/redb are listed.
4. ste-writing rules on all new prose.

**Tests:**
- `context_map_terms_present`: `rg -n 'Redis repository backend|repository service crate|ADR-0063' CONTEXT-MAP.md` → ≥3 hits; `rg -n 'Repository service' crates/services/CONTEXT.md` → ≥1 hit.
  - command: `rg -c 'camel-redis-repo|ADR-0063' CONTEXT-MAP.md crates/services/CONTEXT.md crates/camel-api/CONTEXT.md crates/camel-core/CONTEXT.md`
  - expected: ≥1 per file.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0 (symbol/path citations resolve).

- [x] 3.7

#### Task 3.8: crate CONTEXT.md

**Files:**
- `crates/services/camel-redis-repo/CONTEXT.md` (new)

**Steps:**
1. Write the crate context with these sections (pattern: `crates/components/camel-redis/CONTEXT.md`): Scope boundary (repository service, not a Component — no URI scheme, no endpoints); Language (canonical English, STE); Connection and retry ownership (per-repo MultiplexedExecutor; resolve once; refresh on error; SET NX never re-issued — C1); Sentinel feature posture (always compiled, component feature unification consequence, explicit config selection); Keyspace and clear() safety (prefix grammar `[A-Za-z0-9:_-]`, SCAN+UNLINK, FLUSH forbidden, invalidate_prefix step-prefix guard); Credential redaction (ADR-0051, redacting Debug); Test seams (`FakeRepoExecutor`, `FakeStaticTopology`, camel-test `redis_repositories_test`); Dependency boundary (`camel-api`, `camel-component-redis`, redis-rs as protocol driver — no adapter trait wraps it); Lifecycle/crash ownership (no Consumer task, no background task, eager connect at construction, drop-owned).
2. Cite file paths and Rust symbols (`RedisCacheRepository`, `keyspace::namespaced`, `RepoCommandExecutor`) so `lint-context-citations` resolves them.

**Tests:**
- `crate_context_citations_resolve`: the lint itself is the test.
  - command: `cargo xtask lint-context-citations`
  - expected: exit 0.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.

- [x] 3.8

#### Task 3.9: mdBook documentation updates

**Files:**
- `docs/src/configuration/schema.md` (modified)
- `docs/src/eip/cache.md` (modified)
- `docs/src/eip/idempotent-consumer.md` (modified)

**Steps:**
1. `schema.md`: extend the `cache_repo` reference (~lines 281-289 area for the idempotent/repo config) with the redis backend: `backend = "redis"`, `url` XOR `sentinel_nodes` + `master_name`, `key_prefix` charset, `stale_retention`; extend `idempotent_repo` with the `backend` discriminator (default redb) and redis fields; note `cache_size`/`sweep_interval` are redb-only.
2. `cache.md`: append Redis to the backend list (~lines 51-59) with the cross-process cache sentence and a pointer to ADR-0063.
3. `idempotent-consumer.md`: replace the "choose a durable store" vagueness (~lines 13-17) with the concrete backend list (memory / redb / redis) and the config snippet.
4. Use `{{#include}}` directives pulling the example `Camel.toml` anchors from task 3.10 where a config snippet is shown (docs authoring rule: no inline snippets where a runnable example exists).
5. ste-writing rules on all new prose.

**Tests:**
- `mdbook_build_resolves`: every include/link resolves.
  - command: `nix shell nixpkgs#mdbook -c mdbook build docs`
  - expected: exit 0.

**Acceptance:**
- `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0.

- [x] 3.9

### examples

#### Task 3.10: checked-in redis-repositories example

**Files:**
- `examples/redis-repositories/Cargo.toml` (new)
- `examples/redis-repositories/Camel.toml` (new)
- `examples/redis-repositories/src/main.rs` (new)
- `examples/redis-repositories/README.md` (new)

**Steps:**
1. Package `redis-repositories` following `examples/redis-sentinel/Cargo.toml` (workspace member via the examples glob — verify the glob; if examples are enumerated explicitly, add the entry).
2. `Camel.toml`: `[default.cache_repo] backend = "redis", url = "redis://127.0.0.1:6379", stale_retention = "30m"` and `[default.idempotent_repo] backend = "redis", url = "redis://127.0.0.1:6379"` (database selection goes via the `?db=N` query parameter, db 0 is the default — a `/N` path suffix is rejected by the component URI grammar); anchor the two sections with `# ANCHOR: cache-repo` / `# ANCHOR_END:` ids for the mdBook includes from 3.9.
3. `main.rs`: register a route using the `cache` step with `repository = "redis"` and an `idempotent_consumer` with `repository = "redis"`; env-var overrides for the URL (`REDIS_URL`), mirroring `examples/redis-sentinel/src/main.rs` structure; module docs state prerequisites (a running Redis; no provisioning).
4. `README.md`: run instructions.

**Tests:**
- `example_compiles`: 
  - command: `cargo build -p redis-repositories`
  - expected: exit 0.

**Acceptance:**
- `cargo build -p redis-repositories` exits 0.
- `cargo clippy -p redis-repositories -- -D warnings` exits 0.
- `nix shell nixpkgs#mdbook -c mdbook build docs` still exits 0 (anchors resolve after 3.9 wiring).

- [x] 3.10
