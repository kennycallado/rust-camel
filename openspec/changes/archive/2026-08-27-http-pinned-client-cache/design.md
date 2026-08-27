# Design: http-pinned-client-cache

## Approach

Introduce `PinnedClientCache`, a small `pub(crate)` wrapper around
`moka::future::Cache<PinnedKey, reqwest::Client>` in a new module
`crates/components/camel-http/src/client_cache.rs`. The workspace already
enables moka 0.12 with the `future` feature (the `sync` module is gated
behind a separate feature and is NOT available), so the future flavor is
the zero-friction choice; both call sites (producer send, redirect loop)
are async anyway.

- **Key**: `(host: String, addrs: Vec<SocketAddr>)` with the addr vector
  sorted and deduplicated at key construction (DNS rotation that returns the
  same set in different order must not split cache entries). The key is
  exactly the pair `resolve_initial_url_for_ssrf` already validates per
  request, so a hit means the request's validated resolution equals the
  cached pin under canonical identity — `(host, sorted, deduplicated
  address set)` — and the cached client contains exactly that validated
  set. Reuse is then semantically equivalent to rebuilding —
  DNS-rebinding pinning (L-H2) is preserved because validation stays
  per-request.
- **Value**: `reqwest::Client` built by the existing `build_client(&http_config, Some((host, addrs)))` — unchanged.
- **Bounds**: `PinnedClientCache::new(ttl: Duration, max_entries: u64)` —
  production wiring passes module constants `PINNED_CLIENT_TTL = 60 s` and
  `PINNED_CLIENT_MAX_ENTRIES = 64`; the parameterized constructor keeps
  tests deterministic without clock injection. TTL bounds retained pools,
  TLS material staleness (CA/identity are read from disk at build time),
  and pinned addr sets; capacity bounds fan-out (dynamic hosts via
  `CamelHttpUri`, redirect targets). Eviction follows moka semantics:
  expiry/capacity bound **logical retrievability**; physical reclamation of
  a client happens only after deferred cache maintenance removes the
  cache-owned handle AND the last external clone (in-flight request) drops.
  No strict-LRU victim is promised. Unit tests await `run_pending_tasks()`
  before asserting `entry_count()` (itself approximate).
- **API**: `async fn get_or_build(&self, host: &str, addrs: &[SocketAddr], build: impl FnOnce() -> reqwest::Client) -> reqwest::Client`.
  The cache never sees `HttpConfig`; the production call site passes a
  closure capturing it (`|| build_client(&http_config, Some((host, addrs)))`).
  The cache counts every builder invocation in an internal atomic counter
  exposed as a `#[cfg(test)] pub(crate) fn build_count(&self) -> u64`
  accessor — this makes build
  deltas observable in producer-path and redirect-path in-crate tests,
  where the closure is the fixed production one and cannot be injected. The
  `#[cfg(test)]` gate keeps `clippy -D warnings` clean (no dead_code in
  non-test builds); the atomic counter field itself is production-written
  via `fetch_add`, so it lints clean too.
  moka's `get_with` gives same-key single-flight initialization under
  concurrency.

**Wiring**

1. `HttpEndpoint` gains `pinned_cache: Arc<PinnedClientCache>`, constructed
   in `create_endpoint` (both `HttpComponent` and `HttpsComponent`), cloned
   into every producer so all producers of one endpoint share it. Existing
   direct `HttpEndpoint` struct literals in tests (around `lib.rs:7679+`)
   must add the new field.
2. `HttpProducer` gains `pinned_cache: Arc<PinnedClientCache>`; `call()`
   replaces the per-request `build_client(...)` with a cache lookup.
3. The manual redirect loop in `ssrf.rs` (client rebuild at line ~375)
   receives `&PinnedClientCache` and the endpoint's shared unpinned client
   (it already receives `&HttpConfig`). Redirect hops whose target host is a
   hostname look up per-hop clients in the same cache; hops whose target is
   an IP literal bypass the cache and use the shared unpinned client —
   no DNS, no pin needed, nothing enters the cache.

IP-literal initial URLs (`resolved == None`) keep using the endpoint's shared
client — untouched path.

## Test strategy

Executable test specs (name/arrange/act/assert) are authored in `tasks.md`
per the openspec-task-authoring discipline; the intent inventory and
determinism strategy live here:

- `client_cache` unit tests (module `client_cache.rs`, parameterized short
  TTL ~50 ms, `run_pending_tasks().await` before count assertions):
  `hit_within_ttl_builds_once`, `ttl_expiry_rebuilds`,
  `capacity_bounds_entries`, `addr_order_normalized`,
  `concurrent_same_key_builds_once` (racing `get_or_build` calls, builder
  count == 1).
- Producer wiring: `producers_share_endpoint_cache` (two `create_producer`
  calls from one endpoint fixture → one shared cache, asserted behaviorally via
  `build_count() == 1` after both producers send); producer-path and redirect-path
  behavior exercised in-crate (`#[cfg(test)]` modules in `lib.rs`/`ssrf.rs`)
  against a local test server (pattern already used by
  `ssrf.rs` tests, which spin local listeners) and asserted through the
  cache's `build_count()` deltas: repeated same-host requests grow the
  counter by one; a redirect to a hostname target hits the cache; a
  redirect to an IP-literal target bypasses it (no cache entry for the
  literal key, counter unchanged for it).
- Existing SSRF tests (`ssrf.rs`) must pass unchanged — they are the
  security oracle for per-request validation.

## Affected crates

- `crates/components/camel-http`: new `client_cache.rs` module; `lib.rs`
  (`HttpEndpoint`, `HttpProducer`, both components' `create_endpoint`);
  `ssrf.rs` (redirect hop). `Cargo.toml` adds `moka` (workspace dep, no new
  external dependency). No public API changes.

## Architecture boundaries

Change is entirely inside the HTTP component's data plane (producer send
path). No DSL, runtime, or control-plane surface changes; no new externally
visible types (`pub(crate)` only). Respects the component isolation rules
that keep transport concerns inside `crates/components/*`. CONTEXT-MAP has
no SSRF-specific ADR; the DNS-pinning TOCTOU mitigation is specified in code
comments (L-H2) and pinned by `ssrf.rs` tests, which remain the security
oracle. ADR-0004 (TLS acceptor hot swap) covers the *consumer* side; producer
TLS config is static per endpoint, so no cache-invalidation hook is required —
staleness is bounded by TTL instead.

## Alternatives considered

- **Key by host only** (pin first resolution for TTL): better hit rate under
  DNS rotation, but a request would connect to addresses that were *not* the
  ones validated for that request. Rejected — weakens the pinning invariant.
- **`dashmap` + hand-rolled TTL sweeper**: already a workspace dep, but
  duplicates what moka provides (TTL, capacity, concurrent single-flight
  entry init) with more code to test. Rejected for minimalism.
- **Shared client + global custom resolver** (no per-host clients): loses
  per-request validated pinning entirely — security regression. Rejected.
- **New `HttpConfig` knobs for TTL/capacity**: no demonstrated operator need;
  would ripple into the config JSON schema (`cargo xtask schema --check`).
  Rejected; constants documented in code, revisit if operators ask.
