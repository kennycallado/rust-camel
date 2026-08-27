# Proposal: http-pinned-client-cache

## Why

Production RSS grows ~17-18 MiB/h (+340 MiB over 20 h) on both replicas of a
route fleet using `camel-http` producers (bd: rc-vqqr). The binary uses
jemalloc, so allocator pathology is ruled out. Root cause is in
`HttpProducer::call()` (`crates/components/camel-http/src/lib.rs:2184-2190`):
for every request whose URL has a hostname — `ssrf::resolve_initial_url_for_ssrf`
returns `Some((host, addrs))` for essentially 100% of real traffic — the
producer builds a **new `reqwest::Client`** via `build_client()` instead of
reusing a shared one. Each ephemeral client owns a connection pool, a TLS
context (CA and client identity are read from disk), and pinned DNS
resolution, then is discarded after a single request. Under thousands of
fetches per hour (periodic warmers plus real traffic), client creation
outpaces idle-connection release: sustained, leak-shaped RSS growth. The
redirect path in `ssrf.rs:375` builds a fresh client per hop and has the same
shape at lower volume.

The DNS pinning itself must stay: it closes the DNS-rebinding TOCTOU window
(security note L-H2 in the call-site comment) by resolving and validating
addresses before connect and pinning them with `resolve_to_addrs`.

## What Changes

- Add a bounded cache of pinned `reqwest::Client` instances keyed by
  `(host, resolved addrs)`, using the existing workspace `moka` dependency
  (TTL + capacity bounded).
- `HttpProducer::call()` consults the cache instead of calling
  `build_client()` per request. Per-request SSRF resolution and validation
  (`resolve_initial_url_for_ssrf`) is **unchanged** — a cache hit means the
  request's validated resolution equals the cached client's pin under the
  cache's canonical identity, `(host, sorted and deduplicated address set)`,
  so reuse is exactly as safe as rebuilding.
- The manual redirect loop in `ssrf.rs` uses the same cache for per-hop
  clients.
- IP-literal URLs keep using the endpoint's shared client (unchanged path).

Explicitly excluded: no new `HttpConfig` fields (no config/schema surface
change), no changes to SSRF validation semantics, no consumer-side changes.

## Acceptance criteria

- Repeated requests to the same host with an identical (order-insensitive)
  resolved address set build at most one pinned client while the cached entry remains logically
  retrievable (TTL- and capacity-bounded),
  observed through an injectable builder seam in unit tests.
- Producer-path and redirect-path regression tests prove `HttpProducer::call()`
  and the manual redirect loop obtain clients from the cache: concurrent
  misses on the same key build once, and producers cloned from one endpoint
  share a single cache.
- A resolution change (new addr set) produces a new cached client; the cache
  makes expired entries non-retrievable after a bounded TTL and enforces a
  capacity cap.
- IP-literal URLs never enter the cache — on the initial request and on
  redirect hops alike; literal redirect targets use an unpinned client.
- Existing SSRF tests (`ssrf.rs`) and producer emission tests pass unchanged.
- `cargo fmt --check`, `cargo clippy -p camel-http -- -D warnings`, and
  repo xtask lints pass.

## Risk budget

Acceptable: slightly longer lifetime of idle connections inside a cached
client — bounded by the 60 s TTL (below the 90 s default
`pool_idle_timeout`; a shorter configured idle timeout only closes them
sooner) plus the lifetime of in-flight requests holding client clones;
TTL expiry is logical (non-retrievability) — physical reclamation occurs
only after deferred cache maintenance removes the cache-owned handle and
the last external clone drops. TLS
material staleness is bounded by the same TTL-plus-in-flight window
(producer TLS config is static per endpoint — no hot-reload invalidation
exists today). Out of bounds: any weakening of per-request SSRF validation
or DNS-rebinding pinning; any new public config surface; unbounded
retention.
