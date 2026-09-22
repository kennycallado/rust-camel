# Tasks: rediserr

## Phase 1: Classification core

### Task 1.1: Atomic classifier flip — `transport_error.rs` core, config port, budget marker, fake seam, all synthetic-fixture ports

This task lands rule 6 (plain `ProcessorError` no longer sniffed) AND every seam that depends on it in one atomic change, so the crate lib suite is green at task end.

**Files:**
- `crates/components/camel-redis/src/transport_error.rs` (new)
- `crates/components/camel-redis/src/lib.rs` (modified — add `mod transport_error;`)
- `crates/components/camel-redis/src/config.rs` (modified — delete the substring `is_transient_redis_error` fn, add module-level re-export, port its `is_transient_*` tests)
- `crates/components/camel-redis/src/retry.rs` (modified — budget marker + test fixture ports)
- `crates/components/camel-redis/src/executor.rs` (modified — FakeExecutor seam conversion + fixture ports only; production connect wraps are Task 1.2)
- `crates/components/camel-redis/src/queue.rs` (modified — test fixture ports only; production wraps are Task 2.3)
- `crates/components/camel-redis/src/pubsub.rs` (modified — test fixture ports only; production wraps are Task 2.3)

**Steps:**
1. Create `transport_error.rs` with module docs stating the identity contract (per-site prose audit; rules 1–6 from design.md) and the redis 1.6.0 kind taxonomy rationale.
2. Define the marker types, each `#[derive(Debug)]` + `impl std::fmt::Display` + `impl std::error::Error`:
   - `pub(crate) struct TransientRetryBudgetExhausted { pub stage: String, pub attempts: u32 }`
   - `pub(crate) struct TransportTimeout { pub stage: &'static str }`
   - `pub(crate) struct TransientByProse { pub site: &'static str }`
3. Define the boundary helpers:
   - `pub(crate) fn redis_error_to_camel(op: &str, err: redis::RedisError) -> CamelError` returning `CamelError::ProcessorErrorWithSource(format!("Redis {op} failed: {err}"), std::sync::Arc::new(err))`.
   - `pub(crate) fn redis_error_raw(err: redis::RedisError) -> CamelError` returning `CamelError::ProcessorErrorWithSource(err.to_string(), std::sync::Arc::new(err))` (passthrough sites whose legacy text was bare `{e}`).
   - `pub(crate) fn marker_camel(text: String, marker: impl std::error::Error + Send + Sync + 'static) -> CamelError` returning `CamelError::ProcessorErrorWithSource(text, std::sync::Arc::new(marker))`.
4. Implement `pub(crate) fn legacy_substring_matches(msg: &str) -> bool` — the EXACT legacy word set (`connection`, `io error`, `timed out`, `broken pipe`, `connection reset`, `eof`, `refused`, `readonly`, `read only`) lowercased contains, documented as the rule-5 fallback, the only substring matching in the crate's classification.
5. Implement `pub(crate) fn is_transient_redis_error(err: &CamelError) -> bool`:
   - `Config(_)` or `ConfigValidation(_)` → `false`.
   - `Io(_)` → `true`.
   - Walk `err.source()` chain (bound: 8 hops): if a source downcasts to one of the three markers → `true`.
   - First source (same walk) downcasting to `redis::RedisError` → classify: `Server(ReadOnly)` variant or `ClusterConnectionNotFound` variant → `true`; `Io` kind whose `source()` downcasts to `std::io::Error` with kind `ConnectionRefused | ConnectionReset | ConnectionAborted | BrokenPipe | TimedOut` → `true`; otherwise → `legacy_substring_matches(&redis_err.to_string())`.
   - No marker and no `RedisError` found → `false`.
6. In `config.rs`: DELETE the `is_transient_redis_error` fn and add a module-level `pub use crate::transport_error::is_transient_redis_error;` so the import path `camel_component_redis::config::is_transient_redis_error` stays stable (the path consumer is `camel-redis-repo/src/lib.rs:24`, which does `pub(crate) use camel_component_redis::config::is_transient_redis_error;`); update the section comment `── Transient error detection ──` and doc comments to describe structural classification.
7. In `retry.rs`: change `retry_budget_exhausted` to return `marker_camel(format!("connection lost while {stage} (retry budget exhausted after {} attempts): {cause}", policy.max_attempts), TransientRetryBudgetExhausted { stage: stage.into(), attempts: policy.max_attempts })` — message text byte-identical. Update module docs + fn doc: the ADR-0012 invariant is now the marker; the word "connection" stays in the text for operator continuity, no longer load-bearing.
8. In `executor.rs` FakeExecutor seam (`impl RedisCommandExecutor for FakeExecutor`, the `is_transient` arm at ~line 99): replace `Err(CamelError::ProcessorError(format!("Connection error: {}", fake_err.message)))` with `Err(crate::transport_error::marker_camel(format!("Connection error: {}", fake_err.message), TransientByProse { site: "FakeExecutor transient seam" }))` — message text identical, verdict via marker; the non-transient arm stays plain `ProcessorError`.
9. Port EVERY synthetic transient fixture that feeds classification, preserving each test's intent:
   - `retry.rs` tests at ~102 AND ~122 (BOTH are `CamelError::ProcessorError("connection refused".into())`): each → `redis_error_raw(redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)))`; the `WRONGTYPE` non-transient input at ~148 stays as-is.
   - `queue.rs` tests ~306, ~352, ~379, ~409: `CamelError::ProcessorError("connection reset"/"connection refused".into())` → `redis_error_raw(...)` io-ConnectionReset/ConnectionRefused equivalents.
   - `pubsub.rs` tests ~511, ~571, ~919: same port; where a test's fixture text is asserted verbatim, keep the text by building the redis error with a matching detail via `redis_error_raw(redis::RedisError::from((redis::ErrorKind::Io, "<original text>")))` (General Io Display `<text> - Io` still matches the rule-5 fallback words).
   - `executor.rs` tests using `transient_err(...)` keep using the `FakeError { is_transient: true }` helper — the seam now maps it structurally.
10. Port every `is_transient_*` test in `config.rs`'s test module to structured fixtures per the new test list below. These REPLACE the five config.rs `is_transient_*` tests at ~2044 (`detects_connection_errors`, five asserts incl. `IO error: broken pipe` / `timed out` / `EOF`), ~2063 (`readonly`), ~2070 (`read only`), ~2077 (`rejects_business_errors`: `NOSCRIPT`/`InvalidUri`), ~2097 (`rejects_config_errors_with_transient_substrings`); the business-error rejections migrate to `server_wrongtype_is_not_transient` plus a plain-`ProcessorError`-not-sniffed row, the Config-substring rejections to `config_with_transient_substring_is_not_transient`. No orphan config.rs test may remain referencing the deleted local fn.

**Tests** (in `transport_error.rs` `#[cfg(test)] mod tests`; command `cargo test -p camel-component-redis --lib transport_error` — all fail before step 5 exists):
- `io_error_source_connection_refused_is_transient` — setup: `redis::RedisError` built from `std::io::Error::from(std::io::ErrorKind::ConnectionRefused)` wrapped via `redis_error_to_camel("GET", err)`; action: `is_transient_redis_error(&camel_err)`; assert: `true`.
- `io_error_source_broken_pipe_is_transient` — same shape with `ErrorKind::BrokenPipe`; assert `true`.
- `io_error_source_permission_denied_is_not_transient` — `ErrorKind::PermissionDenied` wrapped; assert `false` (legacy: "permission denied" matched no word).
- `server_readonly_is_transient` — `redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly), "READONLY You can't write against a read only replica."))` wrapped via `redis_error_to_camel`; assert `true`.
- `server_wrongtype_is_not_transient` — server error text "WRONGTYPE Operation against a key holding the wrong kind of value"; assert `false`.
- `server_message_with_classifier_word_falls_back_transient` — server `ResponseError` kind with text "ERR connection lost while processing"; assert `true` via rule-5 fallback.
- `cluster_connection_not_found_is_transient` — `redis::ErrorKind::ClusterConnectionNotFound` General error; assert `true` (its Debug rendering contained "connection").
- `io_general_static_detail_falls_back` — `redis::RedisError::from((redis::ErrorKind::Io, "SSL Handshake error"))` → assert `false` ("ssl handshake error - io" matches no word); twin `redis::RedisError::from((redis::ErrorKind::Io, "connection dropped"))` → assert `true`.
- `tls_close_notify_text_falls_back_transient` — `redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "peer closed connection without sending TLS close_notify"))` (io kind not enumerated → rule-5 on Display, which contains "connection"); assert `true`.
- `camel_error_io_variant_is_transient` — `CamelError::Io("cache serialization: bogus".into())`; assert `true`.
- `plain_processor_error_text_is_not_sniffed` — `CamelError::ProcessorError("connection refused".into())` (no source); assert `false` (the false-positive removal).
- `budget_marker_is_transient` — `marker_camel("connection lost while connecting (retry budget exhausted after 3 attempts): x".into(), TransientRetryBudgetExhausted { stage: "connecting".into(), attempts: 3 })`; assert `true`.
- `timeout_marker_is_transient` — `marker_camel("Redis connection to 'redis://x' timed out after 10s".into(), TransportTimeout { stage: "connect" })`; assert `true`.
- `prose_marker_is_transient` — `marker_camel("failed to build Redis connection info: can't connect with TLS".into(), TransientByProse { site: "topology connection info" })`; assert `true`.
- `config_with_transient_substring_is_not_transient` — `CamelError::Config("CA file at /etc/readonly.pem unreadable".into())`; assert `false`.
- `authentication_failed_is_not_transient` — `redis::ErrorKind::AuthenticationFailed` with text "WRONGPASS invalid username-password pair"; assert `false`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes — the FULL lib suite, green (this is the atomic-flip guarantee).
- `grep -n "msg.contains" crates/components/camel-redis/src/config.rs` returns nothing (legacy body gone).
- The only `contains(` classification code lives inside `legacy_substring_matches`.
- `cargo fmt --check` and `cargo clippy -p camel-component-redis -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: `executor.rs` — connect-timeout marker + source-preserving connect wrap

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified)

**Steps:**
1. Connect-timeout wrap (the `tokio::time::timeout` in `get_conn`, ~line 344): replace `CamelError::ProcessorError(format!("Redis connection to '{}' timed out after {}s", redis_url_safe, timeout_secs))` with `crate::transport_error::marker_camel(<same format text>, TransportTimeout { stage: "executor connect" })`.
2. Connect-error wrap (auth-enriched, ~line 353): keep building the enriched message via `crate::config::enrich_data_auth_error` exactly as today, but return `CamelError::ProcessorErrorWithSource(enriched_text, std::sync::Arc::new(e))` so the `redis::RedisError` is preserved (prose "Failed to connect to Redis at" contains no classifier word — audit row satisfied by source preservation).
3. Update the existing dead-port connect test asserting `matches!(result, Err(CamelError::ProcessorError(_)))` at ~line 733 to also accept `ProcessorErrorWithSource(_, _)`, keeping the assertion that the error is not `Config`.

**Tests** (command `cargo test -p camel-component-redis --lib executor`; suite green before and after — behavior-preserving):
- `test_retry_succeeds_after_transient_failures` — existing, green through the seam from Task 1.1.
- Dead-port connect test (step 3) — updated pattern, still passing.
- `connect_timeout_classifies_transient` (new) — construct the marker-wrapped timeout error via the same `marker_camel` call shape as step 1; assert `is_transient_redis_error` → `true`.

**Acceptance:**
- `cargo test -p camel-component-redis --lib executor` passes.
- No `ProcessorError(format!("Redis connection to` remains in `executor.rs` (timeout site uses marker).
- `cargo fmt --check` and `cargo clippy -p camel-component-redis -- -D warnings` exit 0.

- [x] 1.2

## Phase 2: Boundary sweep

### Task 2.1: `commands/*.rs` — mechanical source-preserving swap (9 files)

**Files:**
- `crates/components/camel-redis/src/commands/hash.rs` (modified)
- `crates/components/camel-redis/src/commands/key.rs` (modified)
- `crates/components/camel-redis/src/commands/list.rs` (modified)
- `crates/components/camel-redis/src/commands/mod.rs` (modified)
- `crates/components/camel-redis/src/commands/other.rs` (modified)
- `crates/components/camel-redis/src/commands/pubsub.rs` (modified)
- `crates/components/camel-redis/src/commands/set.rs` (modified)
- `crates/components/camel-redis/src/commands/string.rs` (modified)
- `crates/components/camel-redis/src/commands/zset.rs` (modified)

**Steps:**
1. In each file, replace every `.map_err(|e| CamelError::ProcessorError(format!("Redis {OP} failed: {e}")))` (all spellings: inline, multiline, `format!` with extra args kept verbatim) with `.map_err(|e| crate::transport_error::redis_error_to_camel(OP, e))`, where `OP` is the exact command token already in the message so the produced text is byte-identical.
2. Any site whose wrap text differs from the `Redis {OP} failed: {e}` shape (e.g. bare `{e}` or extra context words): check the text for legacy classifier words — if none, preserve via `crate::transport_error::redis_error_raw(e)` or keep the exact format string and return `ProcessorErrorWithSource(<exact same text>, Arc::new(e))`; if a classifier word is present in static prose, use `crate::transport_error::marker_camel(<exact same text>, TransientByProse { site: "<file> <op>" })`. Record each non-standard row as a comment `// audit: <verdict rationale>` at the site.
3. Do NOT touch sites converting non-redis errors (e.g. header-validation `ProcessorError`s built from strings) — those contain no `redis::RedisError` and stay plain.

**Tests** (command `cargo test -p camel-component-redis --lib commands`):
- `command_wrap_preserves_redis_source` (add in `commands/mod.rs` tests) — setup: a `redis::RedisError` io-ConnectionRefused; action: call one representative command-module wrap through its normal path with a failing fake executor, then `is_transient_redis_error` on the error; assert: `true` AND the message is exactly `Redis <OP> failed: <redis display>` AND `err.source()` downcasts to `redis::RedisError`.
- Existing command-module unit tests keep passing (no text changes).

**Acceptance:**
- `grep -rn 'ProcessorError(format!("Redis' crates/components/camel-redis/src/commands/` returns zero hits.
- `cargo test -p camel-component-redis --lib` passes.
- `cargo fmt --check` and `cargo clippy -p camel-component-redis -- -D warnings` exit 0.

- [x] 2.1

### Task 2.2: `topology.rs` — resolve/connect boundary wraps per audit table

**Files:**
- `crates/components/camel-redis/src/topology.rs` (modified)

**Steps:**
1. Site "failed to build Redis connection info: {e}" (~line 122): return `crate::transport_error::marker_camel(<same text>, TransientByProse { site: "topology connection info" })` — legacy always-transient via the word "connection".
2. Sites "failed to open Redis client: {e}" (~lines 140, 144, 203) and "failed to build sentinel client: {e}" (~line 347): prose has no classifier word — return `ProcessorErrorWithSource(<same text>, Arc::new(e))`.
3. Site "sentinel resolve join: {e}" (~line 494): prose has no classifier word, inner is a tokio `JoinError` — return `ProcessorErrorWithSource(<same text>, Arc::new(e))` (verdict stays `false` for panic/cancel texts, identical to legacy).
4. The auth-enriched sentinel resolve wrap following it (rc-swzq prose): keep the enriched text verbatim; return `ProcessorErrorWithSource(enriched, Arc::new(e))` preserving the `redis::RedisError`.
5. Verify `topology_tests.rs` assertions (`!is_transient_redis_error` at ~516, ~786, ~920) still pass unchanged — they assert Config-family errors, covered by rule 1.

**Tests** (command `cargo test -p camel-component-redis --lib`):
- `connection_info_failure_is_transient_by_prose` — setup: build the wrap exactly as step 1 does with a non-transient inner text (e.g. "can't connect with TLS, the feature is not enabled"); action: classify; assert: `true`.
- `client_open_failure_classifies_by_inner_kind` — `ProcessorErrorWithSource("failed to open Redis client: x", Arc::new(redis_io_refused))` → `true`; twin with an `AuthenticationFailed` error → `false`.
- Existing `topology_tests.rs` suite passes unchanged.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes (covers `topology.rs` and `topology_tests.rs` module tests).
- Every `ProcessorError(format!` remaining in `topology.rs` carries an `// audit:` comment.
- `cargo fmt --check` and `cargo clippy -p camel-component-redis -- -D warnings` exit 0.

- [x] 2.2

### Task 2.3: `queue.rs` + `pubsub.rs` — production Io wraps, run-loop audit, repo verification

**Files:**
- `crates/components/camel-redis/src/queue.rs` (modified)
- `crates/components/camel-redis/src/pubsub.rs` (modified)
- `crates/services/camel-redis-repo/src/executor.rs` (modified — test fixtures only, if any synthetic transient `ProcessorError` inputs exist; production code untouched)

**Steps:**
1. `RedisQueueIo::connect` timeout wrap "Queue connection timed out after {}s": `marker_camel(<same text>, TransportTimeout { stage: "queue connect" })`.
2. `RedisQueueIo::connect` error wrap "Failed to create connection: {}": `marker_camel(<same text>, TransientByProse { site: "queue connect" })` (legacy always-transient via "connection").
3. `RedisQueueIo::blpop` no-conn guard "Queue connection not established": `marker_camel(<same text>, TransientByProse { site: "queue blpop guard" })`.
4. `RedisQueueIo::blpop` error passthrough `ProcessorError(e.to_string())`: `crate::transport_error::redis_error_raw(e)`.
5. `RedisPubSubIo::connect` timeout wrap "PubSub connection timed out after {}s": `marker_camel(<same text>, TransportTimeout { stage: "pubsub connect" })`.
6. `RedisPubSubIo::connect` error wrap "Failed to create PubSub connection: {}": `marker_camel(<same text>, TransientByProse { site: "pubsub connect" })`.
7. `RedisPubSubIo::subscribe`/`psubscribe` no-conn guards "PubSub connection not established": `marker_camel(<same text>, TransientByProse { site: "pubsub guard" })`.
8. `RedisPubSubIo::subscribe`/`psubscribe` error wraps "Failed to subscribe to channel/pattern {x}: {e}" (no classifier word): `ProcessorErrorWithSource(<same text>, Arc::new(e))`.
9. Audit the `run_queue_consumer`/`run_pubsub_consumer` loop bodies for any other error construction between the Io trait and `transient_retry_step`; convert per the same rules with `// audit:` comments.
10. Verify `camel-redis-repo` needs no production change: run its unit suite; port fixtures ONLY where tests feed classification synthetic `ProcessorError` strings (expected: none — repo errors are `CamelError::Io`, covered by rule 2; the executor.rs response-timeout fixture at ~line 728 is `CamelError::Io` and stays).

**Tests** (command `cargo test -p camel-component-redis --lib && cargo test -p camel-redis-repo --lib`):
- `queue_connect_timeout_is_transient` — classify the marker-wrapped timeout error; assert `true`.
- `queue_blpop_passthrough_refused_is_transient` — `redis_error_raw(io refused)`; assert `true`; twin with a WRONGTYPE server error asserts `false`.
- `pubsub_connect_failure_is_transient_by_prose` — marker-wrapped "Failed to create PubSub connection: x" carrying a non-transient inner text; assert `true` (legacy prose-word identity).
- `pubsub_subscribe_failure_classifies_by_inner_kind` — `ProcessorErrorWithSource("Failed to subscribe to channel ch: x", Arc::new(io_refused))` → `true`; WRONGTYPE twin → `false`.
- All existing loop tests (reconnect budget, stream-end replay) pass — their fixtures were ported in Task 1.1.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` and `cargo test -p camel-redis-repo --lib` pass.
- No `ProcessorError("connection` string literals remain in queue.rs/pubsub.rs production code.
- `cargo fmt --check`, `cargo clippy -p camel-component-redis -- -D warnings`, `cargo clippy -p camel-redis-repo -- -D warnings` exit 0.

- [x] 2.3

### Task 2.4: Docs — CONTEXT.md ADR-0012 paragraph + design.md audit-table finalization

**Files:**
- `crates/components/camel-redis/CONTEXT.md` (modified)
- `openspec/changes/rediserr/design.md` (modified)

**Steps:**
1. In `CONTEXT.md`, update the "Transient-retry `warn!` sites" paragraph: replace "the word 'connection' in that message is what makes `is_transient_redis_error` classify it transient (ADR-0012)" with the marker regime (`TransientRetryBudgetExhausted` marker; word retained in text for operator continuity only). Update the `RedisEndpointConfig::validate_tls()` paragraph's classifier mention similarly (Config early-return unchanged).
2. In `design.md`, finalize the "Per-site static-prose audit" table with every converted site row from tasks 1.1–2.3 (site, prose, classifier word present?, treatment), PLUS explicit "not on classification path" rows for the known out-of-scope sites: `producer.rs` ~98 ("Redis health check PING failed…" — flows to route error handling, never classified) and `health.rs` ~43/49/59 ("Health check connection to '…' timed out" — flows to `HealthStatus::Unhealthy`, never classified). Confirm the flip list is empty.

**Tests:**
- Documentation task. Verify with `grep -n "load-bearing" crates/components/camel-redis/CONTEXT.md` returning either nothing or only text noting the marker supersedes it.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0 (CONTEXT.md lints clean).
- design.md audit table has a row for every site converted in tasks 1.1, 1.2, 2.1, 2.2, 2.3 plus the two "not on classification path" row groups.


- [x] 2.4