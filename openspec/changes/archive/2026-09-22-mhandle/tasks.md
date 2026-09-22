# Tasks: mhandle

## camel-redis-repo

### Task 1.1: Thread ComponentMetrics through executor + both repositories

**Files:**
- `crates/services/camel-redis-repo/src/executor.rs` (modified)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified)
- `crates/services/camel-redis-repo/src/idempotent_repo.rs` (modified)

**Steps:**
1. In `executor.rs`, change `execute_retry_safe` to
   `pub(crate) async fn execute_retry_safe(ex: &Arc<dyn RepoCommandExecutor>, cmd: redis::Cmd, metrics: &ComponentMetrics, operation: &'static str) -> Result<redis::Value, CamelError>`.
   In the `Err(err) if is_transient_redis_error(&err)` arm, BEFORE `ex.refresh()`, add
   `metrics.observe("redis", operation, true);`. Do not change the non-transient arm or the retry count.
2. In `executor.rs`, change `scan_unlink_pattern` to
   `pub(crate) async fn scan_unlink_pattern(ex: &Arc<dyn RepoCommandExecutor>, pattern: &str, metrics: &ComponentMetrics, operation: &'static str) -> Result<u64, CamelError>`
   and forward `metrics, operation` to every `execute_retry_safe` call inside it (SCAN and UNLINK).
3. In `cache_repo.rs`, add field `metrics: ComponentMetrics` to `RedisCacheRepository`. Add `metrics: ComponentMetrics` as the last parameter of `connect` and `with_executor`; store it. Import `camel_api::ComponentMetrics`.
4. In `cache_repo.rs`, update the internal `execute_retry_safe`/`scan_unlink_pattern` call sites with `&self.metrics` and these bounded literals: `set` storage primitive → `"set"`; `get` → `"get"`; delete/remove → `"remove"`; `clear` → `"clear"`; `invalidate_prefix` → `"invalidate_prefix"`.
5. In `idempotent_repo.rs`, add field `metrics: ComponentMetrics` to `RedisIdempotentRepository`; add the parameter to `connect` and `with_executor`; store it. Update `contains` → `"contains"`, `remove` → `"remove"`, and `clear`/its scan call → `"clear"` at their `execute_retry_safe`/`scan_unlink_pattern` call sites.
6. In `idempotent_repo.rs` `add`'s `Err(err) if is_transient_redis_error(&err)` arm, after the existing `transient_refreshes.fetch_add(1, ...)` line, add `self.metrics.observe("redis", "add", true);`. Keep the atomic counter, the `tracing::debug!`, the refresh, and the `Err(err)` return exactly as they are.
7. Update every existing in-crate test constructor (`RedisCacheRepository::with_executor` calls in `cache_repo.rs` tests, `RedisIdempotentRepository::with_executor` calls in `idempotent_repo.rs` tests) to pass a lever-off facade: `ComponentMetrics::new(std::sync::Arc::new(camel_api::metrics::MetricsHandle::new()), false)` — introduce a small `#[cfg(test)]` helper `fn test_facade() -> ComponentMetrics` in each test module to avoid repetition.

**Tests:** (existing suites, updated for new signatures — all must stay green)
- `set_retries_once_after_transient`: fake `[transient, Ok]` → set succeeds — unchanged behavior, new facade arg.
- `get_err_on_transient_never_silent_miss`: fake `[transient, transient]` → get returns `Err` — unchanged behavior, new facade arg.
- `non_transient_no_retry`: fake `[non-transient]` → single attempt, `Err` — unchanged behavior, new facade arg.
- `transient_add_increments_observability_counter`: fake `[transient]` → add `Err`, `transient_refresh_count() == 1` — atomic counter intact.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0 (all existing tests green).
- `cargo clippy -p camel-redis-repo --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean for the three files.
- NOTE: the public `connect` signature change leaves `camel-config`/`camel-test` non-compiling until Task 2.1 — the workspace build gate lands there deliberately; do not gate mid-sequence.

- [x] 1.1

### Task 1.2: C1 emission evidence tests (fake executor + recording collector)

**Files:**
- `crates/services/camel-redis-repo/src/test_metrics.rs` (new)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified — tests)
- `crates/services/camel-redis-repo/src/idempotent_repo.rs` (modified — tests)
- `crates/services/camel-redis-repo/src/lib.rs` (modified — `#[cfg(test)] mod test_metrics;`)

**Steps:**
1. Create `src/test_metrics.rs` with `pub(crate) struct RecordingMetrics` holding `Mutex<Vec<(String, String, String)>>` for component-ops and `Mutex<Vec<(String, String)>>` for errors. Implement `MetricsCollector`: the five required methods (`record_exchange_duration`, `increment_exchanges`, `set_queue_depth`, `record_circuit_breaker_change`) are no-ops, `increment_errors` (required) pushes `(route_id, error_type)`, and the DEFAULTED `record_component_operation` is overridden to push `(component, operation, outcome)` triples. Add `pub(crate) fn ops(&self) -> Vec<String>` returning formatted `"component:op:outcome"` strings and `pub(crate) fn errors(&self) -> Vec<(String, String)>` clones. `RecordingMetrics` must be `Send+Sync` (shared as `Arc<dyn MetricsCollector>`).
2. Add the `#[cfg(test)] mod test_metrics;` declaration in `lib.rs`.
3. Add the five tests below to the existing test modules (cache tests in `cache_repo.rs`, idempotent tests in `idempotent_repo.rs`), each building the repo via `with_executor` with `ComponentMetrics::new(Arc::new(RecordingMetrics::new() as Arc<dyn MetricsCollector>), lever)` where `lever` is per-test.

**Tests:** (executable spec — name, setup, action, assert)
- `transient_get_records_one_operation_and_error` (cache_repo.rs): fake replies `[Err(transient), Ok(Nil)]`, lever ON → call `get` → assert `Ok(None)` returned, `ops() == ["redis:get:failure"]` exactly (no success entry), `errors() == [("redis", "e:redis:get")]`.
- `transient_add_records_failure_and_keeps_atomic_counter` (idempotent_repo.rs): fake `[Err(transient)]`, lever ON → call `add` → assert `Err` returned, `ops() == ["redis:add:failure"]`, `errors() == [("redis", "e:redis:add")]`, `transient_refresh_count() == 1`.
- `non_transient_error_records_nothing` (cache_repo.rs): fake `[Err(non-transient)]` (mirror the non-transient error string used by `non_transient_no_retry`), lever ON → call `get` → assert `Err` returned AND `ops().is_empty()` AND `errors().is_empty()`.
- `clear_transient_unlink_batch_records_clear_observation` (cache_repo.rs): fake `[Ok(scan page with ≥1 key), Err(transient), Ok(int)]`, lever ON → call `clear` → assert `Ok` returned, `ops() == ["redis:clear:failure"]`, `errors() == [("redis", "e:redis:clear")]` — one observation per transient classification even though SCAN also succeeded.
- `lever_off_suppresses_operations_not_errors` (cache_repo.rs): fake `[Err(transient), Ok(Nil)]`, lever OFF → call `get` → assert `ops().is_empty()` AND `errors() == [("redis", "e:redis:get")]`.

**Acceptance:**
- `cargo test -p camel-redis-repo` exits 0 with the five new tests passing.
- `cargo clippy -p camel-redis-repo --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-metric-labels` exits 0 (all emission is facade-mediated with call-site literals).

- [x] 1.2

## camel-config + camel-test

### Task 2.1: Builders construct the facade from the shared handle + lever snapshot

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)
- `crates/camel-test/tests/redis_tls_test.rs` (modified)
- `crates/camel-test/tests/redis_sentinel_tls_test.rs` (modified)

**Steps:**
1. In `context_ext.rs`, add `use camel_api::ComponentMetrics;` (camel-api is already a dependency).
2. Add helper `fn redis_repo_component_metrics(metrics: std::sync::Arc<dyn camel_api::MetricsCollector>, config: &CamelConfig) -> ComponentMetrics` returning `ComponentMetrics::new(metrics, config.observability.metrics.components_enabled())` with a doc comment stating it snapshots the `[observability.metrics].components` lever from the same config the context itself snapshots.
3. Change `build_redis_cache_repo(ccfg: &CacheRepoConfig, metrics: ComponentMetrics)` and `build_redis_idempotent_repo(icfg: &IdempotentRepoConfig, metrics: ComponentMetrics)` to take the facade and pass it into the respective `connect` calls as the new last argument.
4. At both call sites inside `configure_context_with_beans`, build the facade via `redis_repo_component_metrics(ctx.metrics(), config)` and pass it to the builders. Do not touch `wrap_disk_offload`.
5. In `crates/camel-test/tests/redis_tls_test.rs` (2 call sites) and `redis_sentinel_tls_test.rs` (1 call site), append `ComponentMetrics::new(std::sync::Arc::new(camel_api::metrics::MetricsHandle::new()), false)` as the last `connect` argument (add the `use` lines). These suites stay `#[ignore]`d live tests — compile-only fix.

**Tests:** (executable spec)
- `redis_repo_component_metrics_gates_operations_by_lever` (new test in `context_ext.rs` tests): build a `MetricsHandle` and construct the facade FIRST via `redis_repo_component_metrics(handle.clone() as Arc<dyn MetricsCollector>, &config)` — THEN register a `RecordingCollector` (local test struct implementing `MetricsCollector` recording ops+errors) on the handle, and call `facade.observe("redis", "get", true)` — this order exercises the late-bound contract (a collector registered after facade construction is still observed). For `config_with_lever(true)` and `config_with_lever(false)` (CamelConfig with `[observability.metrics].components` set via `MetricsLeversConfig`) → assert lever ON records one op `("redis","get","failure")` + error `("redis","e:redis:get")`; lever OFF records only the error.
- Existing `configure_context*` tests: `cargo test -p camel-config --lib` all green (builders' new parameter is internal; no config surface changed).

**Acceptance:**
- `cargo test -p camel-config --lib` exits 0 including the new lever test.
- `cargo test -p camel-redis-repo` still exits 0.
- `cargo clippy -p camel-config -p camel-test --all-targets -- -D warnings` exits 0.
- `cargo build --workspace` exits 0 (camel-test TLS suites compile).

- [x] 2.1
