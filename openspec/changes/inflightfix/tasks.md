# Tasks: inflightfix

## camel-component-http (crates/components/camel-http)

### Task 1.1: bound helper + four seam validations + tests

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add `pub(crate) fn max_inflight_requests_limit(configured: usize) -> Result<usize, CamelError>` next to `envelope_channel_capacity` (lib.rs ~1779). Body: if `configured > tokio::sync::Semaphore::MAX_PERMITS`, return `Err(CamelError::Config(format!("maxInflightRequests {configured} exceeds the supported upper bound {} (tokio::sync::Semaphore::MAX_PERMITS)", tokio::sync::Semaphore::MAX_PERMITS)))`; otherwise `Ok(configured)`. Do NOT normalize 0 (rc-3y6j zero-reject semantics; grpc's `.max(1)` is intentionally not mirrored). Include a doc comment citing bd rc-ns3yc and the mirror commit 101327e5.
2. Parse seam: in `HttpServerConfig::from_components` (lib.rs ~772-774), immediately after the existing `max_inflight_requests` let-binding that ends in `.unwrap_or(1024)`, add `let max_inflight_requests = max_inflight_requests_limit(max_inflight_requests)?;`. `from_uri_with_defaults` inherits via its call to `from_components`.
3. create_consumer seam: in `HttpEndpoint::create_consumer` (the `impl Endpoint for HttpEndpoint` block, lib.rs ~2761), before the final `Ok(Box::new(HttpConsumer::new(self.server_config.clone(), rt)))` statement, add `max_inflight_requests_limit(self.server_config.max_inflight_requests)?;`.
4. Start seam: in `HttpConsumer::start` (lib.rs ~1807), as the first statement, add `max_inflight_requests_limit(self.config.max_inflight_requests)?;` — before the `ServerRegistry::global().get_or_spawn` call.
5. Defense-in-depth: in `spawn_entry` (lib.rs ~1229), as the FIRST statement — before the listener bind/`local_addr`/registry/CancellationToken work and immediately guarding `tokio::sync::Semaphore::new` (lib.rs ~1256) — add `max_inflight_requests_limit(max_inflight_requests)?;`.
6. Field doc: extend the `max_inflight_requests` field doc (lib.rs ~691) with: bd rc-ns3yc — values above `tokio::sync::Semaphore::MAX_PERMITS` are rejected at parse, create_consumer, start, and spawn_entry; `0` remains a representable reject-everything value.
7. Write the tests below in the crate's `tests` module FIRST, run them, and confirm the marked ones are red (panic, wrong result, or no-config-error); then apply steps 1-6 until green.

**Tests:** (executable spec — name, arrange, act, assert)
- `test_max_inflight_requests_limit_boundary`: `L = tokio::sync::Semaphore::MAX_PERMITS`; call `max_inflight_requests_limit` with `L-1`, `L`, `L+1`, `0` → assert `Ok(L-1)`, `Ok(L)`, `Err(CamelError::Config(msg))` where `msg` contains `"maxInflightRequests"`, the value `L+1`, and the limit `L`, and `Ok(0)` (no normalization). Also construct `let _ = tokio::sync::Semaphore::new(L);` in the same test to prove the accepted bound is constructible without panic (spec scenario "accepted boundary is constructible in Tokio primitives"). Command: `cargo test -p camel-component-http test_max_inflight_requests_limit -- --nocapture`. Expected: fails to compile before step 1 (red via compilation); passes after.
- `test_parse_http_uri_max_inflight_at_limit_accepted`: `L = MAX_PERMITS`; parse `http://localhost:8080/api?maxInflightRequests={L}` via the same parse entry the existing tests at lib.rs ~6953 use → assert `Ok` and `config.max_inflight_requests == L`. Command: `cargo test -p camel-component-http test_parse_http_uri_max_inflight_at_limit`. Expected: passes before AND after (boundary-regression guard; red-first not definable).
- `test_parse_http_uri_max_inflight_above_limit_rejected`: parse `http://localhost:8080/api?maxInflightRequests={L+1}` → assert `Err(CamelError::Config(msg))` with `msg` containing `"maxInflightRequests"`, `L+1` decimal, and `L` decimal. Command: `cargo test -p camel-component-http test_parse_http_uri_max_inflight_above_limit`. Expected: RED before step 2 (parse currently returns Ok).
- `test_create_consumer_rejects_oversized_max_inflight`: construct the component with a directly-built `HttpServerConfig` (struct literal, `http` scheme, no TLS) with `max_inflight_requests: L + 1` (mirror the direct-endpoint fixture style of the existing create_consumer tests); call `create_consumer` → assert `Err(CamelError::Config(msg))` containing param+value+limit. Command: `cargo test -p camel-component-http test_create_consumer_rejects_oversized_max_inflight`. Expected: RED before step 3 (currently returns Ok).
- `test_http_consumer_start_rejects_oversized_before_spawn`: build `HttpConsumer::new` with `max_inflight_requests: L + 1` (mirror the construction fixture of `test_http_consumer_start_with_zero_max_inflight_rejects_503`, lib.rs ~7999, including `ServerRegistry::reset()` hygiene if that test uses it); call `start(ctx)` → assert `Err(CamelError::Config(msg))` containing param+value+limit, and no panic. Command: `cargo test -p camel-component-http test_http_consumer_start_rejects_oversized`. Expected: RED before step 4 (currently panics inside `spawn_entry` at `Semaphore::new`).
- `test_spawn_entry_rejects_oversized_before_semaphore`: stage a pre-bound `tokio::net::TcpListener` on `127.0.0.1:0`; call `spawn_entry(key, ListenerSource::Staged(listener), 1, 1, L + 1, runtime, "route-test".into(), None)` using the crate's existing test runtime fixture (`NoopRuntimeObservability` / `test_rt()` around lib.rs ~4000) → assert `Err(CamelError::Config(msg))` containing param+value+limit, and no panic. Command: `cargo test -p camel-component-http test_spawn_entry_rejects_oversized`. Expected: RED before step 5 (currently panics at `Semaphore::new`).
- Regression guard: `cargo test -p camel-component-http test_http_consumer_start_with_zero_max_inflight_rejects_503` must remain green (zero semantics unchanged). Expected: green before and after.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 (all new + existing tests pass).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- `grep -n 'Semaphore::new' crates/components/camel-http/src/lib.rs` shows no non-test construction without a preceding `max_inflight_requests_limit` call on the same value.
- The four rejection tests above were observed red (panic/wrong result) before the seam patches, or red-first was recorded as not-compilable/not-definable per test.

- [x] 1.1

### Task 1.2: docs — CONTEXT.md + README.md bound notes

**Files:**
- `crates/components/camel-http/CONTEXT.md` (modified)
- `crates/components/camel-http/README.md` (modified)

**Steps:**
1. `CONTEXT.md`: in the in-flight semaphore paragraph (lines ~15-18), append one sentence: values above `tokio::sync::Semaphore::MAX_PERMITS` are rejected with a typed configuration error at parse, create, start, and spawn seams (bd rc-ns3yc, mirroring rc-9kgtm); `0` keeps its reject-everything meaning.
2. `README.md`: in the parameter table row for `maxInflightRequests` (line ~46), append to the description: upper bound `tokio::sync::Semaphore::MAX_PERMITS` (2^61-1 on 64-bit targets); oversized values fail configuration with a typed error instead of panicking.

**Tests:**
- Docs-only task — no test cases. Verify with `cargo xtask lint-context-citations` (CONTEXT.md citation lint) and manual diff read.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0 (run from worktree root).
- `grep -n 'MAX_PERMITS' crates/components/camel-http/CONTEXT.md crates/components/camel-http/README.md` shows the new bound note in both files.
- No other doc sections touched (diff limited to the two named edits).

- [x] 1.2
