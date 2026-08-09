# Design: audit-fix-misc-correctness

## Approach

Seven independent fixes, each isolated to one crate. No cross-crate
dependencies. Each fix is a targeted code change plus a regression test.

### Fix 1 — camel-log char-semantics truncation (rc-3smd)

`lib.rs:356-360` calls `body_str.truncate(limit)` where `limit` is the
`max_chars` config value. The field is named `max_chars` and documented
as a character count, but the current guard `body_str.len() > limit`
checks bytes and `truncate(limit)` operates on bytes. This is a
semantic mismatch: a 4-char string with multibyte content (e.g.
`"日本語"`, 9 bytes) would be truncated mid-sequence and panic.

**Fix:** Commit to char semantics. Replace the byte-length guard and
`truncate` with `body_str.chars().take(limit).collect::<String>()`.
This is a behavior change: strings are now truncated to N characters,
not N bytes. The existing test `max_chars: Some(10)` asserting
`.len() <= 10` must be updated to `.chars().count() <= 10`. The test
name `test_log_config_max_chars_param` stays.

### Fix 2 — camel-component-seda concurrent forwarders (rc-exa2)

`SedaConsumer::start()` in `SedaMode::Single` spawns exactly one forwarder
regardless of `concurrent_consumers`. The `forwarder_handles: Vec<JoinHandle>`
machinery is populated with one handle.

**Fix:** After taking the receiver out of the endpoint mutex (as today),
wrap it in `Arc<tokio::sync::Mutex<mpsc::Receiver>>` and spawn
`concurrent_consumers` forwarder tasks. Each forwarder acquires the lock,
calls `recv()`, then drops the guard BEFORE calling `forward_envelope()`.
This achieves true parallelism for the expensive `send_and_wait` (InOut)
call: the lock is held only for the receive (nanoseconds when messages
are available), not for the processing. When the channel is empty, the
lock serializes idle forwarders — but there is no work to parallelize
in that state.

The `std::sync::Mutex` currently used for `Arc<Mutex<Option<Receiver>>>`
in `SedaMode::Single` stays — it guards only the `take()` call (no await
held). A new `tokio::sync::Mutex` wraps the receiver for the N-forwarder
sharing.

`background_task_handle()` returns `self.forwarder_handles.pop()` (one
handle for Runtime supervision). The remaining handles are cancelled in
`stop()` — this is the existing contract and is unchanged.

The `Fanout` branch is unchanged (one forwarder per subscriber channel).

### Fix 3 — camel-proto-compiler unique descriptor path (rc-gr8k)

`compiler.rs:32-35` writes to `temp_dir().join(format!("camel-proto-{N}.desc"))`
where `N` is a process-local `AtomicU64`. Two processes hitting counter `N`
clobber the same file.

**Fix:** Replace the static counter with `tempfile::NamedTempFile`.
NamedTempFile is guaranteed unique by the OS and is deleted on drop.
The `static COMPILE_COUNTER` is removed. The `cache.rs` layer that reads
the descriptor file after compilation is unaffected — it reads from the
path returned by the temp file handle. After reading, the temp file is
dropped and cleaned up by the OS.

### Fix 4 — camel-container cleanup respects docker_host (rc-xvuk)

`cleanup_tracked_containers()` at `lib.rs:61` is a free `pub async fn`
that calls `Docker::connect_with_local_defaults()`, ignoring the
configured `docker_host`. The connection logic that respects `docker_host`
lives on `ContainerGlobalConfig` (`connect_docker_client()` at lib.rs:458).

**Fix:** Extract the socket-connection logic from
`ContainerGlobalConfig::connect_docker_client` into a standalone helper
`fn connect_docker_from_host(docker_host: Option<&str>) -> Result<Docker,
CamelError>`. This helper parses the host string the same way
`docker_socket_path()` does (rejecting non-unix/npipe schemes) and falls
back to `connect_with_local_defaults()` when `None`. Both
`cleanup_tracked_containers` and `connect_docker_client` call this helper.

The `cleanup_tracked_containers` signature gains
`docker_host: Option<&str>`. Callers that have a config pass
`Some(&config.docker_host)`; callers without config pass `None`.

### Fix 5 — camel-ws TLS readiness deferral (rc-jh8s)

`WsConsumer::start()` calls `ctx.mark_ready()` at `lib.rs:1008`
unconditionally. For plain `ws://`, `TcpListener::bind` runs before spawn,
so readiness is accurate. For `wss://`, `axum_server::bind_rustls` runs
inside the spawned task (lib.rs:228-244) and `.serve().await` resolves
only on termination — so readiness is premature.

**Fix:** Use `axum_server::Handle` (axum-server 0.7 API). Create
`let handle = axum_server::Handle::new();` and call
`handle.listening()` to obtain a future that resolves to
`Option<SocketAddr>` — `Some(addr)` when the listener is bound,
`None` on bind failure. Pass the handle to the server via
`.handle(handle)`. The spawned task runs `.serve().await` (blocks until
shutdown). Outside the spawned task, await `listening` — when it returns
`Some(_)`, the TLS listener is bound and `mark_ready()` is safe to call.
When it returns `None`, propagate the bind failure to `start()` as an error.

For the TLS branch in `spawn_server`:
1. Create `axum_server::Handle::new()`, get `listening = handle.listening()`.
2. Spawn the server task with `.handle(handle).serve(app).await`.
3. Return `listening` (a `Future<Output = Option<SocketAddr>>`) to the caller.
4. In `start()`, await `listening`: on `Some(_)` call `mark_ready()`; on `None` return an error.

The plain `ws://` path keeps its current synchronous `TcpListener::bind`
followed by `mark_ready()` — no change.

### Fix 6 — camel-bean #[non_exhaustive] (rc-sfy1)

`BeanError` (`error.rs:5-23`) is a pub enum returned by `register()` and
re-exported from `lib.rs`. Adding a variant post-1.0 is breaking.

**Fix:** Add `#[non_exhaustive]` to the enum. Internal construction sites
are unchanged. External `match` expressions require a `_ =>` arm. The
crate has 23 `#[test]` functions (13 in registry.rs, 6 in
bean_macro_test.rs, 2 in error.rs, 2 in handler_parsing_test.rs); all
compile unchanged because they construct variants, not exhaustively match.

### Fix 7 — camel-endpoint-macros trybuild suite expansion (rc-7ka6)

The crate ALREADY has 4 trybuild compile-fail cases in `tests/ui/`:
`kind_typo_fail.rs`, `no_optin_no_metadata_fn_fail.rs`,
`secret_with_default_fail.rs`, `unknown_key_fail.rs`. The harness is
`tests/ui_tests.rs`. The audit identified 15 `syn::Error::new` /
`to_compile_error` sites in the source, of which only 4 are covered.

**Fix:** Add 3+ new `*_fail.rs` / `*.stderr` pairs in `tests/ui/`
covering error paths not yet locked:
- Missing `#[uri_scheme = "..."]` attribute on the struct
- Non-struct input (enum) — error message is `"UriConfig only supports
  structs with named fields"` at `uri_config.rs:510`
- Duplicate path field — error message is `"only one field can be the
  path field"` at `uri_config.rs:590`

The existing `ui_tests.rs` harness auto-discovers `tests/ui/*_fail.rs`,
so no harness change is needed. Run `TRYBUILD=overwrite` once to
generate the `.stderr` snapshots, then review them.

## Affected crates

- `camel-log`: char-semantics truncation in `format_exchange`
- `camel-component-seda`: concurrent forwarder spawn in `SedaConsumer::start`
- `camel-proto-compiler`: unique descriptor path in `compile_proto`
- `camel-container`: docker_host-aware cleanup in `cleanup_tracked_containers`
- `camel-ws`: deferred TLS readiness via `axum_server::Handle::listening()`
- `camel-bean`: `#[non_exhaustive]` on `BeanError`
- `camel-endpoint-macros`: trybuild compile-fail suite expansion

## Architecture boundaries

All seven fixes respect the Runtime / DSL / Components / Services / Languages
/ Functions split. No change crosses a layer boundary:

- camel-log, camel-component-seda, camel-ws: Component layer internal fixes.
- camel-proto-compiler: Service-layer build utility fix.
- camel-container: Component-layer runtime utility fix.
- camel-bean: Component-layer public enum stability.
- camel-endpoint-macros: proc-macro test-coverage addition.

ADR-0049 (`#[non_exhaustive]` policy) applies to the camel-bean fix. ADR-0007
(Consumer supervision) is respected by the SEDA concurrency change — the
Runtime supervises via `background_task_handle()` which returns one handle,
while `stop()` aborts all remaining handles.

## Alternatives considered

- **SEDA concurrency via broadcast channel:** Rejected — broadcast duplicates
  messages, which is fanout semantics. The fix must deliver each envelope to
  exactly one forwarder.
- **SEDA lock held across forward_envelope:** Rejected — serializes
  processing. The lock-drop-before-process pattern is required for true
  parallelism.
- **SEDA std::sync::Mutex for receiver sharing:** Rejected — the guard is
  not `Send` and cannot be held across `.recv().await`.
- **Proto-compiler PID-only naming:** Rejected — PID alone can collide if a
  process is forked. NamedTempFile is cleaner and auto-cleans.
- **WS readiness via oneshot after serve().await:** Rejected — `.serve().await`
  resolves only on termination. `axum_server::Handle::listening()` is the
  correct API for bind-success detection.
- **WS readiness via health-probe-only:** Rejected — health probes are
  reactive; readiness must be deterministic at startup.

## Related decisions

- ADR-0049: workspace `#[non_exhaustive]` policy (applies to camel-bean)
- ADR-0007: Consumer task failure supervision (SEDA forwarder handles)
- ADR-0019: `poll_ready` ready, failures in `call()` (SEDA forwarder contract)
