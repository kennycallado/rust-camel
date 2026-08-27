# Proposal: http-component-shared-client

## Why

Change `http-shared-pinned-cache` (4e70eb88) made the pinned-client cache
component-scoped, but `create_endpoint` still builds the shared unpinned
`reqwest::Client` per call (`build_client(&self.config, None)`,
`lib.rs:1914` http, `lib.rs:1993` https). On dynamic EIP paths
(`recipientList`/`routingSlip`/`dynamicRouter` resolve per exchange) that
is one throwaway client per exchange: a TLS context, CA and client-identity
PEM reads from disk, and a hyper pool, all discarded when the endpoint
drops. For hostname traffic this client is never used — the pinned path
serves those requests. The demo fleet's dynamic `recipientList` still pays
this per exchange (bd rc-gdgs).

The config-identity argument is already validated twice for the pinned
cache: the client derives solely from the component's `HttpConfig`
(`self.config`), so all endpoints of one component instance are
config-identical and sharing is exact.

## What Changes

- `HttpComponent` and `HttpsComponent` each gain one
  `client: reqwest::Client` field, built once in their constructors via
  `build_client(&self.config, None)`.
- `create_endpoint` passes `self.client.clone()` (cheap Arc-handle clone)
  into `HttpEndpoint` instead of building a client.
- A `#[cfg(test)]` invocation counter on `build_client` (same philosophy
  as `PinnedClientCache`'s `build_counter`) makes sharing observable in
  unit tests without sending requests.
- One sentence in `crates/components/camel-http/CONTEXT.md` noting the
  shared unpinned client also holds up to `pool_max_idle_per_host` idle
  connections for its hosts.

Excluded: the behavioral HTTPS pinned-cache test (bd rc-0li3); PEM re-read
cadence note (bd rc-mgki); any change to `build_client` semantics, SSRF
validation, or pinning.

## Acceptance criteria

- Component construction builds exactly one unpinned client
  (`build_client` counter delta == 1 per component constructor call).
- `create_endpoint` adds zero `build_client` invocations; a dynamic-style
  resolution loop over distinct URIs adds zero.
- `reqwest::Client` clone-per-endpoint shares one pool per component
  instance (no per-endpoint pool).
- Existing 264-test suite passes unchanged; `cargo fmt --check`,
  `cargo clippy -p camel-component-http -- -D warnings`, and repo xtask
  lints pass.

## Risk budget

Acceptable: wider pool sharing for the unpinned client — it serves IP-literal
requests and IP-literal redirect hops, now through one pool per component
instance instead of one per endpoint. Total pools shrink; no unbounded
retention path appears. Out of bounds: any change to `build_client`
construction semantics, SSRF validation, DNS pinning, or the pinned cache;
new public API; any `camel-core` change.
