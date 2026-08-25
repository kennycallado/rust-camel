# Tasks: fix-redis-db-url

## camel-component-redis

### Task 1.1: StandaloneTopology builds ConnectionInfo directly

**Files:**
- `crates/components/camel-redis/src/topology.rs` (modified)
- `crates/components/camel-redis/src/pubsub.rs` (modified — one test-module caller)
- `crates/components/camel-redis/CONTEXT.md` (modified — one description line)

**Steps:**
1. Un-gate the helper `node_redis_connection_info(config: &RedisEndpointConfig) -> redis::RedisConnectionInfo` (currently `#[cfg(feature = "sentinel")]`, ~L334-345): remove the cfg attribute so both standalone and sentinel share one credential/db builder. Its `set_username` arm is a no-op for standalone configs (`from_uri` never sets `username` for standalone), so behavior is unchanged for sentinel and nothing new is introduced for standalone. Keep `sentinel_node_conn_info` (its caller) sentinel-gated as today.
2. In `topology.rs`, change `StandaloneTopology` to store `addr: redis::ConnectionAddr` and `settings: redis::RedisConnectionInfo` instead of `url: String`.
3. Replace `pub fn new(url: impl Into<String>) -> Self` with `pub fn new(config: &RedisEndpointConfig) -> Self`:
   - `let host = config.host.clone().unwrap_or_else(|| "localhost".into());` and `let port = config.port.unwrap_or(6379);` (mirror `redis_url()` defaults).
   - addr: `redis::ConnectionAddr::Tcp(host.clone(), port)` when `!config.is_ssl_enabled()`, else `redis::ConnectionAddr::TcpTls { host: host.clone(), port, insecure: false, tls_params: None }` (legal construction: only the enum is `#[non_exhaustive]`; the variant and its fields are public).
   - settings: `node_redis_connection_info(config)` (the un-gated helper from step 1 — this literally unifies standalone and sentinel on the same db/credential mechanism, as design.md records).
4. In `impl RedisTopology for StandaloneTopology::resolve`, assemble per call (no `.expect(`/`.unwrap()` — `lint-unwrap` bans both in production source):
   `let info = self.addr.clone().into_connection_info().map_err(|e| CamelError::ProcessorError(format!("failed to build Redis connection info: {e}")))?.set_redis_settings(self.settings.clone());` then `redis::Client::open(info)` with the existing error mapping.
5. In `topology_from_config`, change the standalone arm to `StandaloneTopology::new(config)`.
6. Update the in-crate test caller `pubsub.rs` test `standalone_pubsub_stream_end_returns_err_on_budget` (~L492): `RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=SUBSCRIBE").expect("valid uri")` then `StandaloneTopology::new(&cfg)` (keep the test's original URL semantics). Also update any `StandaloneTopology::new` URL-string call in `topology.rs`'s own `mod tests` (~L365). The `consumer.rs` test `standalone_consumer_uses_standalone_topology` needs NO edit — it reaches the topology via `RedisConsumer::new`/`topology_from_config` and serves as the consumer-path verification.
7. Add a short doc comment on `StandaloneTopology` explaining WHY the info is built structurally: redis-rs parses `?db=` only for unix-socket URLs; for TCP the db rides the path segment, so a `?db=N` URL string silently drops db (bd rc-c5l7).
8. Update `crates/components/camel-redis/CONTEXT.md` (~L81): change the `StandaloneTopology` bullet from "returns a client for one fixed URL for both `ServerKind::Master` and `ServerKind::Replica`" to describe structurally built connection information: "returns a client for one fixed, structurally built connection (address, database, credentials) for both `ServerKind::Master` and `ServerKind::Replica`".

**Tests:** (executable spec — name, setup, action, assert, command, expected)
- `standalone_topology_carries_configured_db`: `RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&db=2")` → `StandaloneTopology::new(&cfg)` → `resolve(ServerKind::Master).await` → assert `client.get_connection_info().redis_settings().db() == 2`. Command: `cargo test -p camel-component-redis --lib standalone_topology_carries_configured_db`. Expected: compile-red against the old constructor; green after the refactor.
- `standalone_topology_default_db_zero`: `from_uri("redis://localhost:6379?command=GET")` (no db param) → resolve → assert `redis_settings().db() == 0`. Command: `cargo test -p camel-component-redis --lib standalone_topology_default_db_zero`. Expected: compile-red against the old constructor; green after.
- `standalone_topology_tls_addr_keeps_db`: `from_uri("rediss://localhost:6380?command=GET&db=3")` (`from_uri` sets ssl from the `rediss` scheme) → resolve → assert `matches!(info.addr(), redis::ConnectionAddr::TcpTls { insecure: false, .. })` AND `redis_settings().db() == 3`. `Client::open` only parses; no TLS feature or network needed. Command: `cargo test -p camel-component-redis --lib standalone_topology_tls_addr_keeps_db`. Expected: compile-red against the old constructor; green after.
- `standalone_topology_password_raw`: `from_uri("redis://localhost:6379?command=GET&password=p@ss:word")` (raw password in the query param — `@` and `:` survive the param splitting and from_uri stores query-param passwords undecoded) → resolve → assert `redis_settings().password() == Some("p@ss:word")` (raw value rides the settings struct; no percent-encode/decode on the driver path). Command: `cargo test -p camel-component-redis --lib standalone_topology_password_raw`. Expected: compile-red against the old constructor; green after.
- Existing `standalone_consumer_uses_standalone_topology` (consumer.rs) must still pass unchanged.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0 (covers the 4 new tests + existing consumer/pubsub/topology tests).
- Existing display-string and round-trip tests pass UNMODIFIED (spec scenario "display strings unchanged"): `cargo test -p camel-component-redis --lib redis_url` and `cargo test -p camel-component-redis --test config_roundtrip`.
- `cargo fmt --check` clean; `cargo clippy -p camel-component-redis --all-features -- -D warnings` exits 0; `cargo xtask lint-unwrap` reports no new violations.
- `redis_url()`, `redis_url_safe()`, `safe_endpoint()`, and `from_uri()` are not edited (git diff shows no hunks in those functions).

- [x] 1.1

## camel-test

### Task 2.1: repository service db regression (testcontainers, end-to-end SELECT 2)

**Files:**
- `crates/camel-test/tests/redis_repositories_test.rs` (modified)

**Steps:**
1. Add test `cache_repo_writes_to_configured_db_live` inside the file's existing `#![cfg(feature = "integration-tests")]` gate, reusing the file's helpers: `own_redis()`, `cache_repo_toml(url, stale_retention)`, `load_camel_toml` (via `context_with_redis_cache`), `cache_entry`, `raw_connection`.
2. Arrange: `let (_container, base_url) = own_redis().await;` (bind as `_container` so the container stays alive for the test duration without an unused-variable warning) then `let db_url = format!("{base_url}?db=2");` plus `let mut verifier_db2 = raw_connection(&format!("{base_url}/2")).await;` and `let mut verifier_db0 = raw_connection(&base_url).await;`.
3. Act: build the context via `context_with_redis_cache(&db_url, stale_retention)` with the same stale-retention value `cache_roundtrip_and_ttl` uses, then put one `cache_entry()` through the context's cache repository (mirror the put call of `cache_roundtrip_and_ttl`).
4. Assert (this observes the `SELECT 2` the executor's connection issues at handshake — keys must land in db 2, not db 0): `redis::cmd("DBSIZE").query_async::<i64>(&mut verifier_db2).await` returns `>= 1` AND `redis::cmd("DBSIZE").query_async::<i64>(&mut verifier_db0).await` returns `== 0`. Raw verifier URLs use the redis crate's native `/N` path-segment parsing, independent of the code under test.
5. Check Docker availability (`docker info` exits 0). If available, run the test; if not, compile-verify only and report `integration-verification-defer-to-CI`.

**Tests:**
- `cache_repo_writes_to_configured_db_live`: fresh redis container → cache repo configured with `?db=2` → one cache put → DBSIZE(db 2) ≥ 1 AND DBSIZE(db 0) == 0. Command (note the feature gate — without it the file compiles to zero tests): `cargo test -p camel-test --features integration-tests --test redis_repositories_test cache_repo_writes_to_configured_db_live -- --nocapture` (requires Docker; else the compile fallback `cargo test -p camel-test --features integration-tests --test redis_repositories_test --no-run`). Expected: red before task 1.1's fix (key lands in db 0 so the DBSIZE(db 2) assertion fails), green after.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test redis_repositories_test --no-run` exits 0.
- If Docker available: the new test passes and at least one pre-existing `*_live` test in the same file still passes.
- `cargo fmt --check` clean.

- [x] 2.1
