# Proposal: http-shared-pinned-cache

## Why

Change `http-pinned-client-cache` (ff7334d0, bd rc-vqqr) caches DNS-pinned
`reqwest::Client` instances per endpoint. That cache never engages for
dynamic EIP destinations. `recipientList`, `routingSlip`, and
`dynamicRouter` resolve each runtime URI through `make_endpoint_resolver`
(`crates/camel-core/src/lifecycle/adapters/endpoint_resolver_factory.rs`).
Each resolution calls `create_endpoint` on the component. Each
`create_endpoint` call in `camel-http` constructs a new `PinnedClientCache`
(`lib.rs:1902` http, `lib.rs:1972` https). Every distinct dynamic URI
therefore gets a fresh, empty cache and a fresh client per request. This
repeats the exact pattern rc-vqqr fixed, plus one moka allocation per
resolution (bd rc-l67y).

A demo route fleet drives a dynamic `recipientList` with
`simple: "${exchangeProperty.originUrl}..."`. Its traffic shape is the one
the fix cannot help. This is no regression versus pre-fix behavior, because
that path already built clients per exchange. But the fix's coverage claim,
"essentially 100% of hostname traffic", holds only for static `to:`
endpoints.

## What Changes

- `HttpComponent` and `HttpsComponent` each gain one
  `pinned_cache: Arc<PinnedClientCache>` field, built when the component is
  constructed. `create_endpoint` clones the `Arc` into the endpoint instead
  of building a cache per endpoint.
- The registry stores one active component per scheme, and registration can
  replace it (`Registry::register`, `registry.rs:44`). Sharing scope is the
  component instance: every endpoint created by one component instance
  shares its cache, so all endpoints and dynamic resolutions served by that
  instance reuse one cache. A replacement component owns a separate cache,
  and existing endpoints keep the cache they were built with.
- Operator note in `crates/components/camel-http/CONTEXT.md`: a shared cache
  retains up to `PINNED_CLIENT_MAX_ENTRIES` clients. Each cached client can
  hold up to `pool_max_idle_per_host` (default 100) idle connections per
  distinct host until `pool_idle_timeout` (default 90 s). Many distinct
  hosts raise the idle-connection footprint.

Excluded: memoizing endpoints inside the resolver factory (cross-component
`camel-core` change); the per-endpoint shared unpinned client, which dynamic
paths still rebuild per resolution (pre-existing); the PEM re-read cadence
note (bd rc-mgki).

## Acceptance criteria

- Two endpoints created from one component through distinct URIs share one
  cache. Requests with the same validated `(host, addrs)` build at most one
  pinned client, observed through the cache's build counter.
- A simulated dynamic-resolution sequence (repeated `create_endpoint` plus
  `create_producer` for distinct URIs that share one host) hits the shared
  cache: one build, cache reused.
- `HttpsComponent` behaves the same.
- Existing pinned-client-cache tests pass unchanged, because they inject a
  shared cache.
- `camel-http` CONTEXT.md carries the idle-connection note.
- `cargo fmt --check`, `cargo clippy -p camel-component-http -- -D
  warnings`, and repo xtask lints pass.

## Risk budget

Acceptable: cross-endpoint cache reuse. Client construction closes over the
endpoint's `http_config`, which is a clone of the component's `HttpConfig`
(`lib.rs:1916`, `lib.rs:1986`). All endpoints of one component therefore
build config-identical clients, so reuse across endpoints is exact. Out of
bounds: any change to SSRF validation or pinning semantics; new config
surface; any `camel-core` change.
