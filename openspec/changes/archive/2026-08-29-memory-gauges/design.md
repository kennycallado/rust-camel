# Design: memory-gauges

## Context

`PinnedClientCache` (crates/components/camel-http/src/client_cache.rs:50) is a
moka cache of DNS-pinned `reqwest::Client` keyed by `(host, validated addr set)`,
TTL 60 s, capacity 64, owned per `HttpComponent` (`Arc<PinnedClientCache>`).
After the proliferation fixes (2026-08-27) it is shared per component; the only
instrumentation is a `build_counter: AtomicU64` exposed `#[cfg(test)]`. Every
client resolution funnels through `get_or_build` (L69–82, moka `get_with`
single-flight) — the natural choke point — and is reached from the producer
steady path (lib.rs:2252–2256) and the SSRF redirect hops (ssrf.rs:389).

jemalloc is opted into only by the `camel-cli` binary
(`#[global_allocator]` under `#[cfg(feature = "jemalloc")]`, main.rs:4–9;
`stats` feature already enabled per Cargo.toml:97 for rc-vnm8 soak
observability). `tikv-jemalloc-ctl` is not yet a dependency anywhere.

Metrics infrastructure: dedicated typed trait methods with closed label sets
are the house style for gauges (`set_queue_depth`, `set_route_state`,
`record_uptime` — metrics.rs:22/53/67). The label lint
(scripts/xtask/src/lint_metric_labels.rs) skips `impl MetricsCollector` blocks
and enum `.as_str()` derivations are approved (L123–133). Prometheus families
live in crates/services/camel-prometheus/src/metrics/families.rs (queue-depth
GaugeVec precedent L149–156). ADR-0066 governs collector binding/lifetime:
`ctx.metrics()` returns an `Arc<dyn MetricsCollector>` pointing at the shared
late-bound handle (context.rs:53/658). The cache wires that Arc once, at
`create_endpoint` time, through a `OnceLock` (idempotent first-wins); the
stored Arc keeps late binding, so emission before collector registration is
a silent no-op with no Option plumbing at the access path.

## Goals

- rc-u4qz: live hit/miss/size series for every `PinnedClientCache`, per HTTP
  component (`camel-http`, `camel-https`), sufficient to detect a
  proliferation regression (size climbing) or a reuse regression (misses
  tracking hits 1:1 under steady traffic).
- rc-0sxi: allocated/resident/active/mapped series when the `jemalloc`
  feature is enabled, sufficient to distinguish live allocation growth
  (allocated growing) from allocator retention (allocated flat while
  resident grows) during soak.

## Non-Goals

- No per-host or per-endpoint labels (cardinality; host values are an open
  set — the class rc-xl5k closed as accepted design).
- No sampler task for the cache (size is emitted opportunistically on access).
- No mimalloc/other-allocator coverage; no allocator choice change (rc-vnm8
  soak decides adoption, not this change).
- No new metrics lever: gauges are unconditional like queue depth.
- No OTEL-backend implementation for the four families: the OTEL collector's
  methods default to no-op and the specs scope emission to the wired
  Prometheus collector. OTEL parity is a possible later task.
- No backfill of historical cache stats; series start at process start.

## Decisions

### D1 — Hit/miss via per-call init-executed flag; direct +1 increments

Every `get_or_build` call creates a local `AtomicBool built = false` captured
by the `get_with` init future. The future that actually runs sets it before
building. After `get_with` resolves: `built == true` → this call constructed
the client → emit `increment_pinned_client_cache_miss(kind)` (+1); otherwise
emit `increment_pinned_client_cache_hit(kind)` (+1). Single-flight makes this
exact: N concurrent cold-key waiters produce 1 miss and N−1 hits; miss counts
client constructions (the proliferation signal), not waiter count. Direct +1
increments (house style of `increment_errors`); no delta bookkeeping, no
races. Size is emitted opportunistically on every access: moka
`entry_count()` is an O(1) approximate read → `set_pinned_client_cache_size`.

Alternative rejected (F1 of spec grill): "emit counter deltas
(current − last_emitted)" — race-prone under concurrent accesses and saves no
calls since emission still happens per access.

Alternative rejected: a detached size sampler per component (queue-depth
pattern) — `HttpComponent` owns no `CancellationToken` lifecycle, and size
under no traffic carries no leak signal.

### D2 — The cache wires its metrics handle at endpoint creation; emission at the choke point

`HttpComponent::new()`/`with_config()` are runtime-less (25+ external call
sites in camel-test alone — signatures MUST NOT change). Instead,
`PinnedClientCache` keeps its current constructor and gains
`wire(kind: HttpComponentKind, metrics: Arc<dyn MetricsCollector>)` — a
`OnceLock`-guarded one-time wiring called from each component's
`create_endpoint(uri, ctx)` (lib.rs:1922), where `ctx.metrics()` yields the
Arc pointing at the shared late-bound handle (context.rs:53/658; registration
at :531 — late binding survives the `Arc<dyn>` coercion because the Arc still
points at the one handle). Before wiring (or never wired — non-endpoint unit
tests), emission is a silent no-op, never a panic. All emission happens inside
`get_or_build`, so every call site (steady producer path, SSRF redirect hops)
inherits identical instrumentation with zero per-site code.
`HttpComponentKind` is a two-variant camel-http enum (`Http`, `Https`) with
`as_str() -> "camel-http" | "camel-https"`; an invariant test asserts the
as_str image is exactly those two literals (the label value is closed by
construction, and provably so).

### D3 — Dedicated typed trait methods, not record_counter strings

Four new `MetricsCollector` methods with intrinsic closed labels:
`set_pinned_client_cache_size(&self, component: &str, entries: u64)`,
`increment_pinned_client_cache_hit(&self, component: &str)`,
`increment_pinned_client_cache_miss(&self, component: &str)`,
`set_allocator_memory(&self, stat: AllocatorStat, bytes: u64)`.
`AllocatorStat` (camel-api, `Copy`, `as_str()` →
`allocated|resident|active|mapped`) keeps every call site lint-clean without
annotations. Precedent: `set_queue_depth` (dedicated method, closed set) vs
`record_counter` (open strings, lint-scrutinized, and
`metrics-contract-hardening` bounds dynamic-name collectors — dedicated names
sidestep that entirely). Default trait impls are no-ops; `MetricsHandle` and
`CompositeMetricsCollector` forward (same shape as every existing method).
The Prometheus backend types the four families once; the OTEL backend keeps
the no-op defaults (documented Non-Goal).

### D4 — jemalloc sampler lives in camel-cli's `run` command, 5 s const interval

`main.rs` has no `CamelContext`; the sampler starts in `commands/run.rs` after
context construction and `ctx.start()`, capturing a clone of the runtime
metrics handle, under `#[cfg(feature = "jemalloc")]`. It is scoped to
`camel run`: other subcommands (validate, lint, bean gen) do not run routes
and get no sampler. The loop (queue-depth sampler shape, seda lib.rs:512–529):
`tokio::spawn` + `tokio::time::interval(Duration::from_secs(5))`, daemon task
terminated by process exit — the run future owns the runtime until exit.

jemalloc-ctl mechanics: `tikv-jemalloc-ctl = "0.7"` (must match
tikv-jemallocator 0.7 — a later 0.8 bump must bump both; comment at the
dependency site) as an optional dependency gated by the existing `jemalloc`
feature. MIBs (`epoch`, `stats::allocated`, `stats::resident`,
`stats::active`, `stats::mapped`) are initialized once before the loop; each
tick advances `epoch` FIRST (jemalloc caches stats between epoch
advancements — reading without advancing returns stale values), then reads
the four MIBs. Failure policy: initialization failure or a read/epoch error
logs `warn` and retries on the next tick; it never aborts `camel run`.
Libraries stay allocator-agnostic; without the feature no jemalloc-ctl code
compiles.

Interval 5 s const, unconditional (no lever): memory stats do not churn at
sub-second scale; one loop of five O(1) reads justifies no config surface
(queue-depth sampler precedent).

### D5 — Sampler test seam: a snapshot read function, not a closure of emissions

The loop body is extracted as
`fn emit_allocator_snapshot(read: impl Fn() -> Result<AllocatorSnapshot, String>, metrics: &Arc<dyn MetricsCollector>) -> bool`
with `struct AllocatorSnapshot { allocated: u64, resident: u64, active: u64,
mapped: u64 }`. `Ok` → four `set_allocator_memory` emissions, one per stat,
exact values, returns true; `Err` → no emission, warn log, returns false.
The cfg-gated production path supplies `read` as "advance epoch, read four
MIBs" and ignores the bool (retry next tick). Unit tests (no `jemalloc`
feature needed): an `Ok` stub asserts the four emissions carry the stub's
exact values; an `Err` stub asserts zero emissions and a false return; an
unwired `MetricsHandle` (ADR-0066 default cell) asserts emission is a silent
no-op, not a panic — the late-binding behavior the run command relies on when
no collector backend is configured.

## Risks / Trade-offs

- **Series go stale when idle**: hit/miss/size series flatline between
  accesses. Accepted: Prometheus `rate()` over a stale counter reads as
  zero-flow, which is truthful, and leaks regress under traffic.
- **moka `entry_count()` is approximate**. The leak signal is a monotonic
  climb, not an exact count — tolerance documented in the spec scenario.
- **jemalloc-ctl version skew**: locked in lockfile; bump-together note at
  the dependency site (D4).
- **OTEL backend no-ops the four families**: scoped in specs to the wired
  Prometheus collector; OTEL parity is an explicit Non-Goal, not an accident.
- **Camel-http emission before metrics wiring**: late-bound handle makes
  early emission a silent no-op (ADR-0066), identical to every component.

## Phases

Two delivery phases, vertically complete each (no half-landed families):

- **Phase 1 — Pinned client cache gauges (rc-u4qz)**: camel-api trait methods
  (cache trio) + forwarding; camel-prometheus cache families; camel-http
  `HttpComponentKind` + handle-carrying cache + choke-point emission; unit
  tests (hit/miss single-flight exactness, invariant as_str image, recording
  collector assertions) — all green with the default feature set.
- **Phase 2 — jemalloc allocator gauges (rc-0sxi)**: `AllocatorStat` enum +
  `set_allocator_memory` + forwarding (camel-api), allocator family
  (camel-prometheus), `tikv-jemalloc-ctl` optional dep + `AllocatorSnapshot`
  seam + `run`-command sampler (camel-cli); unit tests via the seam (Ok/Err/
  unwired-handle), plus a cfg-gated compile check that the default build
  contains no jemalloc-ctl code. Exit criteria: default workspace builds and
  tests stay green without the feature; `--features jemalloc` build compiles
  the sampler; the seam tests prove emission mapping and failure policy.
