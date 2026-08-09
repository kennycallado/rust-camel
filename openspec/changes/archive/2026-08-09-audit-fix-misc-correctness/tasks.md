# Tasks: audit-fix-misc-correctness

## camel-log

### Task 1.1: Char-semantics truncation in LogProducer

**Files:**
- `crates/components/camel-log/src/lib.rs` (modified)

**Steps:**
1. In `format_exchange`, locate the truncation block at lib.rs:355-360 that reads `if let Some(limit) = self.config.max_chars && body_str.len() > limit { body_str.truncate(limit); }`.
2. Replace the byte-based guard and `truncate` call with char-based truncation: `if let Some(limit) = self.config.max_chars { body_str = body_str.chars().take(limit).collect(); }`. Remove the `body_str.len() > limit` guard — `chars().take(limit)` handles both short and long strings correctly.
3. Update the existing test `test_log_truncates_large_body` (lib.rs:633) to assert `body_part.chars().count() <= 10` instead of `body_part.len() <= 10`.
4. Add a new test `test_log_truncates_multibyte_body` that constructs a `LogProducer` with `max_chars: Some(3)` and a body of `"日本語测试"` (5 chars, 15 bytes), formats the exchange, and asserts the extracted body part has exactly 3 characters (`.chars().count() == 3`) and is valid UTF-8.
5. Add a new test `test_log_truncates_multibyte_no_panic` that constructs a `LogProducer` with `max_chars: Some(4)` and a body of `"café"` (4 chars, 5 bytes), formats the exchange, and asserts no panic and the body has exactly 4 characters.

**Tests:**
- `test_log_truncates_large_body`: LogProducer max_chars=10, body="a".repeat(100) → body_part.chars().count() <= 10
- `test_log_truncates_multibyte_body`: LogProducer max_chars=3, body="日本語测试" → body_part has 3 chars, valid UTF-8
- `test_log_truncates_multibyte_no_panic`: LogProducer max_chars=4, body="café" → body_part has 4 chars, no panic
- `test_log_config_max_chars_param`: unchanged — verifies maxChars=50 parsed correctly

**Acceptance:**
- `cargo test -p camel-component-log --lib` passes all tests
- `cargo clippy -p camel-component-log -- -D warnings` exits 0
- No `String::truncate` call remains in format_exchange (grep-verified)

- [x] 1.1

## camel-component-seda

### Task 2.1: Concurrent forwarder spawn in SedaConsumer::start

**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)

**Steps:**
1. Add `use tokio::sync::Mutex as AsyncMutex;` to the imports at the top of the file (line 10 area). The existing `std::sync::{Arc, Mutex}` stays — it guards the `Option<Receiver>` `take()` (no await held).
2. In the `SedaMode::Single` branch of `SedaConsumer::start()` (lib.rs:596-628), after taking the receiver out of the endpoint mutex (line 605-610), wrap it in `Arc<AsyncMutex<mpsc::Receiver<ExchangeEnvelope>>>` via `let shared_rx = Arc::new(AsyncMutex::new(receiver));`.
3. Replace the single `tokio::spawn` (line 614-626) with a `for _ in 0..self.state.config.concurrent_consumers` loop that spawns one forwarder per iteration. Each forwarder clones `shared_rx`, `cancel`, and `ctx` before spawning.
4. Inside each spawned forwarder, implement the lock-drop-before-process pattern: acquire `let mut guard = shared_rx.lock().await;`, `tokio::select!` on `guard.recv()` vs `cancel.cancelled()`. When `recv()` returns `Some(envelope)`, exit the block (guard drops), then call `forward_envelope(&ctx, envelope).await` outside the lock scope. When `recv()` returns `None`, return `Ok(())`. When `cancel.cancelled()` fires, return `Ok(())`.
5. Push each spawned `JoinHandle` into `self.forwarder_handles`.
6. Update the existing `concurrency_model()` (lib.rs:692-695) — no change needed, it already reports `Concurrent { max: Some(self.state.config.concurrent_consumers) }`.
7. Add a test `test_seda_concurrent_forwarders_count` that creates a `SedaEndpointState` with `concurrent_consumers: 4`, starts a consumer, and asserts `consumer.forwarder_handles.len() == 4` after `start()` returns.
8. Add a test `test_seda_concurrent_parallel_processing` that creates a SEDA endpoint with `concurrent_consumers: 2`, enqueues 2 InOut envelopes to the channel, uses a consumer whose processing sleeps 100ms per envelope, and asserts both complete within 300ms (parallel). Use `tokio::time::timeout` to measure.
9. Add a test `test_seda_concurrent_consumers_one_still_single` that creates a SEDA endpoint with `concurrent_consumers: 1`, starts a consumer, and asserts `consumer.forwarder_handles.len() == 1`.

**Tests:**
- `test_seda_concurrent_forwarders_count`: concurrent_consumers=4 → forwarder_handles.len() == 4 after start()
- `test_seda_concurrent_parallel_processing`: concurrent_consumers=2, 2 InOut envelopes with 100ms processing each → completes within 300ms
- `test_seda_concurrent_consumers_one_still_single`: concurrent_consumers=1 → forwarder_handles.len() == 1

**Acceptance:**
- `cargo test -p camel-component-seda --lib` passes all tests
- `cargo clippy -p camel-component-seda -- -D warnings` exits 0
- The `Fanout` branch (SedaMode::Fanout) is unchanged in behavior

- [x] 2.1

## camel-proto-compiler

### Task 3.1: Unique descriptor file via NamedTempFile

**Files:**
- `crates/services/camel-proto-compiler/Cargo.toml` (modified)
- `crates/services/camel-proto-compiler/src/compiler.rs` (modified)

**Steps:**
1. In `Cargo.toml`, move `tempfile = { workspace = true }` from `[dev-dependencies]` to `[dependencies]`. The crate needs it at runtime now, not just in tests.
2. In `compiler.rs`, remove the `static COMPILE_COUNTER: AtomicU64 = AtomicU64::new(0);` declaration (line 10) and its `use std::sync::atomic::{AtomicU64, Ordering};` import if no other code uses it.
3. Replace the descriptor file construction at lines 32-35 (`let descriptor_file = std::env::temp_dir().join(format!("camel-proto-{}.desc", COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed)))`) with `let temp_file = tempfile::Builder::new().suffix(".desc").tempfile()?;` — the `#[from] std::io::Error` on `ProtoCompileError::Io` converts automatically via `?`. Extract the path via `let descriptor_file = temp_file.path().to_path_buf();`.
4. Keep the `temp_file` handle alive until after the descriptor is read back into a `DescriptorPool`. After `prost_reflect::DescriptorPool::decode(...)`, the `temp_file` can be dropped (auto-cleanup). Alternatively, `keep()` the temp file if the cache layer needs a stable path — check `cache.rs` to see if it stores the path or the decoded pool. If it stores the decoded pool, drop is safe.
5. Add a test `test_compile_proto_unique_descriptor_path` that calls `compile_proto` twice in the same process and asserts the two descriptor file paths differ.

**Tests:**
- `test_concurrent_compiles_do_not_clobber`: spawn 2+ `compile_proto` futures concurrently on the same `.proto` input, assert all return `Ok` with non-empty `DescriptorPool` — proves no file collision
- `test_descriptor_file_cleaned_up`: after a successful `compile_proto`, assert no `camel-proto-*.desc` files remain in `std::env::temp_dir()` (NamedTempFile auto-cleans on drop)
- Existing proto-compiler tests pass unchanged

**Acceptance:**
- `cargo test -p camel-proto-compiler` passes all tests
- `cargo clippy -p camel-proto-compiler -- -D warnings` exits 0
- No `COMPILE_COUNTER` static remains (grep-verified)
- No `std::env::temp_dir()` direct call for descriptor path (grep-verified)

- [x] 3.1

## camel-container

### Task 4.1: cleanup_tracked_containers respects docker_host

**Files:**
- `crates/components/camel-container/src/lib.rs` (modified)
- `examples/container-nginx/src/main.rs` (modified)
- `examples/container-hot-reload/src/main.rs` (modified)
- `examples/container-example/src/main.rs` (modified)
- `examples/jms-container-example/src/main.rs` (modified)

**Steps:**
1. Extract a free function `fn connect_docker_from_host(docker_host: Option<&str>) -> Result<Docker, CamelError>` near the top of the file (before `cleanup_tracked_containers`). When `docker_host` is `Some(host)`, replicate `docker_socket_path()` logic exactly (lib.rs:437-455): accept schemeless hosts (e.g. `/var/run/docker.sock`), accept `unix://` and `npipe://` schemes, reject other schemes. Call `Docker::connect_with_socket(host, DOCKER_CONNECT_TIMEOUT_SECS, bollard::API_DEFAULT_VERSION)` to match existing behavior exactly. When `None`, call `Docker::connect_with_local_defaults()`. Return `Err(CamelError::ProcessorError(...))` on connection failure.
2. Refactor `ContainerGlobalConfig::connect_docker_client()` (lib.rs:458-466) to call `connect_docker_from_host(self.host.as_deref())` instead of duplicating the connection logic.
3. Change `cleanup_tracked_containers` signature from `pub async fn cleanup_tracked_containers()` to `pub async fn cleanup_tracked_containers(docker_host: Option<&str>)`.
4. Replace `Docker::connect_with_local_defaults()` at line 75 with `connect_docker_from_host(docker_host)`. Keep the error logging.
5. Update the doc comment at line 60 to mention the `docker_host` parameter.
6. Update all 4 example binary callers to pass `None`: `examples/container-nginx/src/main.rs:83`, `examples/container-hot-reload/src/main.rs:127`, `examples/container-example/src/main.rs:177`, `examples/jms-container-example/src/main.rs:349`. Each currently calls `cleanup_tracked_containers().await` — change to `cleanup_tracked_containers(None).await`.
7. Add a test `test_cleanup_respects_custom_docker_host` that calls `cleanup_tracked_containers(Some("unix:///nonexistent/docker.sock"))` with a tracked container, asserts no panic, and verifies the cleanup attempted the non-default socket (the connection error message contains the custom path).

**Tests:**
- `test_cleanup_respects_custom_docker_host`: cleanup with Some("unix:///nonexistent/docker.sock") → no panic, error references custom socket
- `test_cleanup_none_uses_defaults`: cleanup with None → connects via defaults, no panic on empty tracker

**Acceptance:**
- `cargo test -p camel-component-container --lib` passes all tests
- `cargo clippy -p camel-component-container -- -D warnings` exits 0
- `connect_docker_from_host` is shared by both `cleanup_tracked_containers` and `connect_docker_client` (grep-verified)

- [x] 4.1

## camel-ws

### Task 5.1: WSS readiness deferred via axum_server::Handle::listening()

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. In `spawn_server` (around lib.rs:190-270), in the TLS branch (lines 217-244), create `let handle = axum_server::Handle::new();` before spawning the server task.
2. Obtain the listening future: `let listening = handle.listening();`.
3. Modify the `axum_server::bind_rustls(parsed_addr, tls_cfg)` call to chain `.handle(handle)` before `.serve(make_service)`.
4. Store the `axum_server::Handle` clone in the `ServerHandle` struct (lib.rs:75-79) via a new field `listening_handle: Option<axum_server::Handle>`, set to `Some(handle)` for the TLS branch and `None` for the plain-WS branch.
5. Update `get_or_spawn` (lib.rs:107,166) to return `Result<(WsAppState, Option<axum_server::Handle>), CamelError>` — it must surface the `listening_handle` from the `ServerHandle` alongside the `WsAppState`. Clone the handle from the `ServerHandle` inside the `OnceCell` initialization path.
6. In `WsConsumer::start()` (around lib.rs:1000-1010), after `get_or_spawn` returns `(state, listening_handle)`: if `Some(handle)`, await `handle.listening()`. On `Some(addr)` → call `ctx.mark_ready()`. On `None` → return `Err(CamelError::EndpointCreationFailed("TLS listener bind failed".to_string()))`. If `None` (plain ws, no handle) → call `ctx.mark_ready()` as today (the synchronous bind already succeeded).
6. Update the comment at lib.rs:1004-1007 to reflect the new behavior.
7. Ensure `axum_server::Handle` is in scope (it is `Clone`). No new imports needed beyond `axum_server`. The `get_or_spawn` return-type change must update all callers (search for `get_or_spawn` in the crate).
8. Add a test `test_wss_bind_failure_does_not_mark_ready` that configures a `wss://` endpoint on an already-bound port, calls `start()`, and asserts the return value is `Err` and that `mark_ready()` was never called (use a mock ConsumerContext that panics on `mark_ready()`).

**Tests:**
- `test_wss_bind_failure_does_not_mark_ready`: wss:// on bound port → start() returns Err, mark_ready never called
- Existing `test_server_bind_error_is_reported` (lib.rs:2672) continues to pass

**Acceptance:**
- `cargo test -p camel-component-ws --lib` passes all tests
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0
- `mark_ready()` is gated behind the listening future for the TLS path (grep-verified)

- [x] 5.1

## camel-bean

### Task 6.1: Add #[non_exhaustive] to BeanError

**Files:**
- `crates/camel-bean/src/error.rs` (modified)

**Steps:**
1. In `error.rs`, add `#[non_exhaustive]` on the line immediately before `pub enum BeanError {` (line 5). The attribute goes between the `#[derive(Debug, Error)]` and the `pub enum` declaration.
2. Run the existing test suite to verify all 23 tests pass unchanged.
3. Verify no external match expressions exist in the workspace that exhaustively match `BeanError` without a wildcard arm: `rg -n "match.*BeanError" crates/ --type rust`. If any are found, add a `_ =>` arm.

**Tests:**
- All 23 existing `#[test]` functions in camel-bean pass without modification (13 in registry.rs, 6 in bean_macro_test.rs, 2 in error.rs, 2 in handler_parsing_test.rs)

**Acceptance:**
- `cargo test -p camel-bean` passes all 23 tests
- `cargo clippy -p camel-bean -- -D warnings` exits 0
- `#[non_exhaustive]` is present on `BeanError` (grep-verified)
- `cargo xtask lint-non-exhaustive` passes

- [x] 6.1

## camel-endpoint-macros

### Task 7.1: Expand trybuild compile-fail suite

**Files:**
- `crates/camel-endpoint-macros/tests/ui/missing_uri_scheme_fail.rs` (new)
- `crates/camel-endpoint-macros/tests/ui/missing_uri_scheme_fail.stderr` (new)
- `crates/camel-endpoint-macros/tests/ui/non_struct_fail.rs` (new)
- `crates/camel-endpoint-macros/tests/ui/non_struct_fail.stderr` (new)
- `crates/camel-endpoint-macros/tests/ui/duplicate_path_field_fail.rs` (new)
- `crates/camel-endpoint-macros/tests/ui/duplicate_path_field_fail.stderr` (new)

**Steps:**
1. Create `tests/ui/missing_uri_scheme_fail.rs`: a struct deriving `UriConfig` WITHOUT a `#[uri_scheme = "..."]` attribute. Include `use camel_endpoint_macros::UriConfig;` and a `#[derive(UriConfig)] struct MissingScheme { path: String }`. The expected error is `"missing #[uri_scheme = \"xxx\"] attribute on struct"` (uri_config.rs:145-149).
2. Create `tests/ui/non_struct_fail.rs`: an enum deriving `UriConfig`. Include `use camel_endpoint_macros::UriConfig;` and `#[derive(UriConfig)] enum NonStruct { Variant }`. The expected error is `"UriConfig can only be derived for structs"` (uri_config.rs:802-805).
3. Create `tests/ui/duplicate_path_field_fail.rs`: a struct with two fields that both lack `#[uri_param]` (making both "path" fields). Include `use camel_endpoint_macros::UriConfig;`, `#[uri_scheme = "dup"]`, `#[derive(UriConfig)] struct DupPath { first: String, second: String }`. The expected error contains `"only one field can be the path field"` or equivalent duplicate-path rejection.
4. Run `TRYBUILD=overwrite cargo test -p camel-endpoint-macros --test ui_tests` to generate the `.stderr` snapshots.
5. Review each generated `.stderr` file to confirm the error message is meaningful and stable. The messages must reference the proc-macro's error, not a secondary compiler error.
6. Run `cargo test -p camel-endpoint-macros` to confirm all ui cases pass (including the 4 existing cases).

**Tests:**
- `missing_uri_scheme_fail.rs` → compile fails with "missing uri_scheme" error
- `non_struct_fail.rs` → compile fails with "only be derived for structs" error
- `duplicate_path_field_fail.rs` → compile fails with "only one path field" or equivalent error
- All 4 existing cases continue to pass

**Acceptance:**
- `cargo test -p camel-endpoint-macros` passes all tests including trybuild
- `TRYBUILD=overwrite` was used once, then reviewed — no overwrite flag needed on subsequent runs
- 7 total ui cases exist in `tests/ui/` (4 existing + 3 new)

- [x] 7.1
