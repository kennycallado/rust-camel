# Tasks: memory-gauges

## Phase 1: Pinned client cache gauges (rc-u4qz)

### Task 1.1: camel-api — cache metric trait methods

**Files:**
- crates/camel-api/src/metrics.rs (modified)

**Steps:**
1. Add three methods to trait `MetricsCollector` (beside `set_queue_depth`, metrics.rs:22), each with a default no-op body: `fn set_pinned_client_cache_size(&self, component: &str, entries: u64)`, `fn increment_pinned_client_cache_hit(&self, component: &str)`, `fn increment_pinned_client_cache_miss(&self, component: &str)`.
2. Forward all three in `MetricsHandle` (the ADR-0066 late-bound cell impl, metrics.rs ~L110+) by delegating to the current collector.
3. Forward all three in `CompositeMetricsCollector` (metrics.rs ~L247) to every inner collector, matching the existing forwarding shape.
4. Run `rg -n "impl MetricsCollector" -g '*.rs'` repo-wide; any OTHER impl outside camel-api, camel-prometheus, and test doubles keeps the no-op default — do not touch it here (camel-prometheus impl lands in Task 1.2).

**Tests:**
- name: `handle_forwards_pinned_cache_trio`
  setup: a `MetricsHandle` wired to a recording collector double that captures `(method, component, value)` triples; handle unwired first.
  action: call the three methods on the handle with component `"camel-https"`, entries `3`.
  assert: recording double holds exactly one capture per method with the passed arguments; calling the trio on an unwired handle neither panics nor records.
  command: `cargo test -p camel-api --lib metrics`
  expected: fails before step 2 (methods absent), passes after.
- name: `composite_forwards_pinned_cache_trio_to_all_collectors`
  setup: `CompositeMetricsCollector` over two recording doubles.
  action: call the three methods once each.
  assert: each double captured all three calls exactly once (no double-emission, no drop).
  command: `cargo test -p camel-api --lib metrics`
  expected: fails before step 3, passes after.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- `rg -n "fn set_pinned_client_cache_size|fn increment_pinned_client_cache_hit|fn increment_pinned_client_cache_miss" crates/camel-api/src/metrics.rs` shows exactly one trait declaration each plus forwarding impls.

- [x] 1.1

### Task 1.2: camel-prometheus — cache families

**Files:**
- crates/services/camel-prometheus/src/metrics/families.rs (modified)
- crates/services/camel-prometheus/src/metrics/mod.rs (modified)
- crates/services/camel-prometheus/src/metrics/tests.rs (modified)

**Steps:**
1. In `families.rs`, register three families beside the queue-depth GaugeVec (families.rs:149–156): `camel_pinned_client_cache_size` as `GaugeVec` with label `component`; `camel_pinned_client_cache_hits_total` and `camel_pinned_client_cache_misses_total` as `CounterVec` with label `component`. Follow the existing registration pattern (same constructor, same label-options shape).
2. In `metrics/mod.rs`, implement the three `MetricsCollector` methods on the Prometheus collector (beside `set_queue_depth` impl, mod.rs:167): gauge sets `entries` on `camel_pinned_client_cache_size{component=…}`; hit/miss increment the matching counter by 1, matching the existing increment style.
3. Run the crate's existing test suite to confirm no family-name collision and registration succeeds.

**Tests:**
- name: `pinned_cache_trio_exports_with_component_label`
  setup: Prometheus collector instance with families registered, export/render path used by the crate's existing family tests (find the queue-depth export test and mirror its harness).
  action: call `set_pinned_client_cache_size("camel-http", 7)`, `increment_pinned_client_cache_hit("camel-http")` twice, `increment_pinned_client_cache_miss("camel-https")` once; render the export.
  assert: export text contains `camel_pinned_client_cache_size{component="camel-http"} 7`, `camel_pinned_client_cache_hits_total{component="camel-http"} 2`, `camel_pinned_client_cache_misses_total{component="camel-https"} 1`.
  command: `cargo test -p camel-prometheus --lib metrics`
  expected: fails before steps 1–2, passes after.
- name: `pinned_cache_families_do_not_collide_with_existing`
  setup: full family registration as in the crate's startup path.
  action: construct the collector the same way production does.
  assert: construction succeeds with no duplicate-metric error; `camel_pinned_client_cache_size` appears exactly once in the rendered families list.
  command: `cargo test -p camel-prometheus --lib metrics`
  expected: fails before step 1 (families absent), passes after.

**Acceptance:**
- `cargo test -p camel-prometheus --lib` exits 0.
- `cargo clippy -p camel-prometheus -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.2

### Task 1.3: camel-http — instrument the cache choke point

**Files:**
- crates/components/camel-http/src/client_cache.rs (modified)
- crates/components/camel-http/src/lib.rs (modified)

**Steps:**
1. In `client_cache.rs`, add `#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub(crate) enum HttpComponentKind { Http, Https }` with `pub(crate) fn as_str(&self) -> &'static str` returning `"camel-http"` for `Http` and `"camel-https"` for `Https`.
2. Keep `PinnedClientCache::new` with its current capacity/TTL signature (external component constructors are runtime-less and MUST NOT change shape). Add field `wired: OnceLock<(HttpComponentKind, std::sync::Arc<dyn camel_api::MetricsCollector>)>` plus `pub(crate) fn wire(&self, kind: HttpComponentKind, metrics: std::sync::Arc<dyn camel_api::MetricsCollector>)` — idempotent (first call wins; later calls are no-ops). Keep `build_count`, `run_pending_tasks`, and `entry_count` `#[cfg(test)]`-gated exactly as they are.
3. Rewrite `get_or_build` (client_cache.rs:69–82): create `let built = AtomicBool::new(false)` per call; inside the `get_with` init future set `built` to true (store with `Ordering::Relaxed`) before building the client; after `await`, if the OnceLock is wired: `built` load true → `increment_pinned_client_cache_miss(kind.as_str())`, else → `increment_pinned_client_cache_hit(kind.as_str())`, then `set_pinned_client_cache_size(kind.as_str(), self.cache.entry_count())` (moka's public `entry_count` on the cache field — NOT the `#[cfg(test)]` wrapper kept by step 2, which stays test-only). If NOT wired, skip emission entirely (no panic). Keep the existing `build_counter` increment inside the init future.
4. In `lib.rs`, inside each component's `create_endpoint(uri, ctx)` implementation (HttpComponent at lib.rs:1922; find HttpsComponent's equivalent with `rg -n "fn create_endpoint" -g '*.rs'`), after `ctx.metrics()` becomes available, add one line: `self.pinned_cache.wire(HttpComponentKind::Http, ctx.metrics());` (respectively `HttpComponentKind::Https`). No constructor signature changes anywhere.
5. Update the existing cache-related test constructions to wire a recording collector where instrumentation is asserted (client_cache.rs tests; lib.rs test constructions at 7778/7803/7834 and 8537/8569/8601; ssrf.rs tests at 616/661 construct the cache directly, unwired, so emission is a no-op — they need no change unless they assert cache internals; run the suite and fix only what breaks).

**Tests:**
- name: `single_flight_cold_key_records_one_miss_and_waiter_hits`
  setup: `PinnedClientCache` wired via `wire(HttpComponentKind::Https, Arc::new(recording_double))`; one cold key. Mirror the existing concurrent test shape at client_cache.rs:234 (`concurrent_same_key_builds_once`) — plain sync build closures, no barrier parking.
  action: spawn 4 concurrent `get_or_build` calls on the same key; await all.
  assert: exactly one `increment_pinned_client_cache_miss` capture and three `increment_pinned_client_cache_hit` captures for the cache's component label; `build_count() == 1`.
  command: `cargo test -p camel-component-http --lib client_cache`
  expected: fails before step 3, passes after.
- name: `warm_key_within_ttl_records_hits_only`
  setup: wired cache with recording double; one key built once (cold), then two further sequential `get_or_build` calls inside the TTL.
  action: perform the two warm calls.
  assert: total captures are one miss then two hits; at least one `set_pinned_client_cache_size` capture with the current `entry_count()`.
  command: `cargo test -p camel-component-http --lib client_cache`
  expected: fails before step 3, passes after.
- name: `unwired_cache_emission_is_silent_noop`
  setup: `PinnedClientCache` constructed with `new` only — never wired.
  action: one cold `get_or_build` and one warm call.
  assert: returns the built client both times; no panic; `build_count() == 1`.
  command: `cargo test -p camel-component-http --lib client_cache`
  expected: guard test — passes before and after the change; fails only if unwired emission panics or errors.
- name: `http_component_kind_as_str_image_is_exactly_two_literals`
  setup: the `HttpComponentKind` enum from step 1.
  action: collect `as_str()` over both variants into a sorted set.
  assert: the set equals `["camel-http", "camel-https"]` — the closed label set the spec requires.
  command: `cargo test -p camel-component-http --lib client_cache`
  expected: fails before step 1, passes after.
- name: `wire_is_idempotent_first_handle_wins`
  setup: cache wired twice — first to a recording double, then to a second recording double.
  action: one cold `get_or_build`.
  assert: only the FIRST double captured the miss; the second captured nothing.
  command: `cargo test -p camel-component-http --lib client_cache`
  expected: fails before step 2, passes after.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 (full crate suite, including the ssrf.rs:616/:661 tests that exercise the instrumented cache incidentally).
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- `rg -n "fn wire|OnceLock" crates/components/camel-http/src/client_cache.rs` shows the wiring mechanism; `rg -n "HttpComponent::new\(" -g '*.rs' -g '!target'` shows zero signature changes (all call sites compile untouched).

- [x] 1.3

## Phase 2: jemalloc allocator gauges (rc-0sxi)

### Task 2.1: camel-api + camel-prometheus — allocator stat family

**Files:**
- crates/camel-api/src/metrics.rs (modified)
- crates/services/camel-prometheus/src/metrics/families.rs (modified)
- crates/services/camel-prometheus/src/metrics/mod.rs (modified)
- crates/services/camel-prometheus/src/metrics/tests.rs (modified)

**Steps:**
1. In `camel-api/src/metrics.rs`, add `#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum AllocatorStat { Allocated, Resident, Active, Mapped }` with `pub fn as_str(&self) -> &'static str` returning `"allocated" | "resident" | "active" | "mapped"`.
2. Add trait method `fn set_allocator_memory(&self, stat: AllocatorStat, bytes: u64)` with a default no-op; forward it in `MetricsHandle` and `CompositeMetricsCollector` exactly like the Task 1.1 trio.
3. In `families.rs`, register `camel_allocator_memory_bytes` as a `GaugeVec` with label `stat` beside the cache families from Task 1.2.
4. In `mod.rs`, implement `set_allocator_memory` on the Prometheus collector: set `bytes` on `camel_allocator_memory_bytes{stat=stat.as_str()}`.

**Tests:**
- name: `allocator_stat_as_str_image_is_closed_set`
  setup: the `AllocatorStat` enum.
  action: collect `as_str()` over all four variants into a sorted set.
  assert: the set equals `["active", "allocated", "mapped", "resident"]`.
  command: `cargo test -p camel-api --lib metrics`
  expected: fails before step 1, passes after.
- name: `handle_and_composite_forward_set_allocator_memory`
  setup: wired `MetricsHandle` over one recording double; `CompositeMetricsCollector` over one recording double.
  action: call `set_allocator_memory(AllocatorStat::Resident, 4096)` on both.
  assert: each double captured exactly one `(stat=Resident, bytes=4096)` emission; unwired handle call neither panics nor records.
  command: `cargo test -p camel-api --lib metrics`
  expected: fails before step 2, passes after.
- name: `allocator_family_exports_four_stats`
  setup: Prometheus collector instance, export harness mirrored from Task 1.2's export test.
  action: set all four stats to distinct values (1000, 2000, 3000, 4000); render.
  assert: export contains `camel_allocator_memory_bytes{stat="allocated"} 1000`, `{stat="resident"} 2000`, `{stat="active"} 3000`, `{stat="mapped"} 4000`.
  command: `cargo test -p camel-prometheus --lib metrics`
  expected: fails before steps 3–4, passes after.

**Acceptance:**
- `cargo test -p camel-api --lib` and `cargo test -p camel-prometheus --lib` exit 0.
- `cargo clippy -p camel-api -p camel-prometheus -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

### Task 2.2: camel-cli — jemalloc sampler behind the feature

**Files:**
- crates/camel-cli/Cargo.toml (modified)
- crates/camel-cli/src/allocator_metrics.rs (new)
- crates/camel-cli/src/commands/run.rs (modified)
- crates/camel-cli/src/lib.rs (modified — declare `mod allocator_metrics;` at the lib crate root, beside `pub mod commands;`; run.rs reaches it as `crate::allocator_metrics`)

**Steps:**
1. In `Cargo.toml`, add `tikv-jemalloc-ctl = { version = "0.7", optional = true, default-features = false, features = ["use_std"] }` to `[dependencies]` and extend the existing `jemalloc` feature (Cargo.toml:140) to include `dep:tikv-jemalloc-ctl`. Add the one-line comment `# must stay in lockstep with tikv-jemallocator 0.7` directly above the dependency.
2. Create `allocator_metrics.rs`, with every item gated `#[cfg(any(test, feature = "jemalloc"))]` (the seam's tests build on the default feature set via `cfg(test)`, and production callers build via the feature — plain un-gated `pub(crate)` items would trip `dead_code` under default-feature clippy), containing:
   - `pub(crate) struct AllocatorSnapshot { pub allocated: u64, pub resident: u64, pub active: u64, pub mapped: u64 }`
   - `pub(crate) fn emit_allocator_snapshot(read: impl Fn() -> Result<AllocatorSnapshot, String>, metrics: &Arc<dyn camel_api::MetricsCollector>) -> bool`: on `Ok(snap)` emit `set_allocator_memory` for each of the four stats with the snapshot values and return true; on `Err(_)` log `warn!("jemalloc stats read failed; retrying next tick")` and return false. No panic path.
   - under `#[cfg(feature = "jemalloc")]`: `pub(crate) fn spawn_allocator_sampler(metrics: std::sync::Arc<dyn camel_api::MetricsCollector>)` — initialize the five MIBs (`epoch`, `stats::allocated`, `stats::resident`, `stats::active`, `stats::mapped`) once; on init error log warn and return without spawning; otherwise `tokio::spawn` an interval loop (`Duration::from_secs(5)`) whose each tick supplies `read` as: advance epoch, read the four MIBs, map errors to `Err(String)`; call `emit_allocator_snapshot(read, &metrics)` ignoring the bool. Note: `ctx.metrics()` returns the `Arc<MetricsHandle>` unsized-coerced to `Arc<dyn MetricsCollector>` (context.rs:53 and :658; registration at :531) — late binding survives the coercion because the Arc still points at the one handle.
3. In `commands/run.rs`, after the `ctx.start()` call site (run.rs:600 area), add under `#[cfg(feature = "jemalloc")]`: `crate::allocator_metrics::spawn_allocator_sampler(ctx.metrics());` — no other subcommand references the sampler.
4. Verify default-build isolation: `cargo tree -p camel-cli --edges normal 2>/dev/null | grep -c jemalloc-ctl` must print `0` with default features.

**Tests:**
- name: `ok_snapshot_maps_to_four_exact_emissions`
  setup: recording `MetricsCollector` double capturing `set_allocator_memory` calls; `read` closure returning `Ok(AllocatorSnapshot { allocated: 11, resident: 22, active: 33, mapped: 44 })`.
  action: call `emit_allocator_snapshot(read, metrics_arc)` where `metrics_arc` is the recording double wrapped as `Arc<dyn camel_api::MetricsCollector>`.
  assert: exactly four captures — `(Allocated, 11)`, `(Resident, 22)`, `(Active, 33)`, `(Mapped, 44)` — values unchanged; return value is true.
  command: `cargo test -p camel-cli --lib allocator_metrics`
  expected: fails before the function exists, passes after.
- name: `err_read_emits_nothing_and_returns_false`
  setup: same double; `read` returning `Err("epoch".into())`.
  action: call `emit_allocator_snapshot(read, metrics_arc)` with the same `Arc<dyn camel_api::MetricsCollector>` wrapping.
  assert: zero `set_allocator_memory` captures; return value false; no panic.
  command: `cargo test -p camel-cli --lib allocator_metrics`
  expected: fails before the function exists, passes after.
- name: `unwired_handle_snapshot_is_silent_noop`
  setup: `emit_allocator_snapshot` called with a `MetricsHandle` collector that is unwired (ADR-0066 default cell) instead of the recording double; ok-stub `read`.
  action: call the seam with the unwired handle wrapped in an Arc.
  assert: returns true; no panic.
  command: `cargo test -p camel-cli --lib allocator_metrics`
  expected: fails before step 2, passes after.
- name: `real_read_closure_advances_epoch_and_returns_snapshot`
  setup: built with `--features jemalloc` (the test itself carries `#[cfg(feature = "jemalloc")]`); the real read closure extracted in step 2.
  action: invoke the read closure once.
  assert: returns `Ok(AllocatorSnapshot)` (the epoch advance inside succeeded and all four MIB reads returned values).
  command: `cargo test -p camel-cli --lib --features jemalloc allocator_metrics`
  expected: fails before step 2 creates the closure (compile), passes after. The
  epoch-first ordering itself is enforced by the D4 read shape and review — a dropped
  epoch advance still returns Ok with stale values, so no unit test can red-flag it.
- name: `default_build_contains_no_jemalloc_ctl`
  setup: clean default-feature build.
  action: `cargo tree -p camel-cli --edges normal 2>/dev/null | grep -c jemalloc-ctl`.
  assert: output is `0` — the dependency does not enter the default graph.
  command: (the action itself, run in the worktree)
  expected: fails (non-zero) if step 1 gates the dep incorrectly, passes after.

**Acceptance:**
- `cargo test -p camel-cli --lib` exits 0 (default features — seam tests run).
- `cargo test -p camel-cli --lib --features jemalloc allocator_metrics` exits 0 (real-read smoke test runs).
- `cargo build -p camel-cli --features jemalloc` exits 0 (sampler compiles).
- `cargo clippy -p camel-cli -- -D warnings` exits 0 (default features).
- `cargo clippy -p camel-cli --features jemalloc -- -D warnings` exits 0 (CI never
  clippy-checks the feature-gated sampler otherwise).
- `cargo tree -p camel-cli --edges normal 2>/dev/null | grep -c jemalloc-ctl` prints `0`.
- `cargo fmt --check --all` exits 0.

- [x] 2.2
