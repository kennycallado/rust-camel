# Design: http-component-shared-client

## Approach

Mirror the pinned-cache hoist for the unpinned client.

1. `HttpComponent` and `HttpsComponent` gain `client: reqwest::Client`,
   initialized in their constructors (`new`, `with_config`;
   `with_optional_config` and `Default` funnel through these) with
   `build_client(&self.config, None)` — the exact call `create_endpoint`
   uses today (`lib.rs:1914`, `lib.rs:1993`).
2. Both `create_endpoint` bodies replace their `build_client` call with
   `client: self.client.clone()`. `reqwest::Client::clone` is a cheap
   Arc-handle clone that shares the connection pool, so every endpoint of
   one component instance uses one pool.
3. Test seam: a `#[cfg(test)] thread_local!` counter (`Cell<u64>`) in
   `lib.rs`, incremented at the top of `build_client` under `cfg(test)`.
   Same philosophy as `PinnedClientCache::build_counter` — count every
   build — but thread-local, because sibling tests construct components on
   parallel threads and a process-global counter would make delta
   assertions flaky. The reader is a `#[cfg(test)] pub(crate) fn
   build_client_call_count() -> u64`. The sharing tests are plain sync
   tests, so each test thread observes only its own increments. Tests
   need no requests — endpoint creation alone must move the counter by
   zero.
4. Consumers of `HttpEndpoint.client`: the producer's IP-literal path and
   `send_with_ssrf_safe_redirects`'s `shared_client` argument (IP-literal
   redirect hops). Both take the endpoint's client; sharing across
   endpoints of one component is config-identical, so behavior is
   unchanged. `create_consumer` (`lib.rs:2027-2047`) touches only
   `server_config` and uses no client.

Config-identity invariant: the unpinned client derives solely from
component `HttpConfig`. No per-URI input reaches `build_client` (the
`resolve_override` argument is `None` at both constructor and endpoint
call sites today). A pool shared across endpoints of one component is
therefore exact, and total retained pools shrink from
endpoints-per-component to one per component instance.

Tests reuse the counter: constructor delta == 1 per component; endpoint
creation delta == 0 across distinct URIs, including a dynamic-resolution
shaped loop (`create_endpoint` + `create_producer`, no requests). The
pinned path's own `build_client` closure (cache miss) never fires without
requests, so counters stay deterministic.

Docs: `crates/components/camel-http/CONTEXT.md` operator note gains one
sentence — the shared unpinned client also holds up to
`pool_max_idle_per_host` idle connections for its hosts.

## Affected crates

- camel-component-http: component structs, `create_endpoint`, `build_client`
  counter seam, unit tests, CONTEXT.md sentence.

## Architecture boundaries

Components layer only. No `camel-core` change, no public API change
(`client` field private, counter `cfg(test)`), no config surface, no SSRF
or pinning semantics change.

## Out of scope, observed but not changed

- Behavioral HTTPS pinned-cache sharing test (bd rc-0li3).
- PEM re-read cadence note (bd rc-mgki). Note: this change reduces PEM
  reads on dynamic paths from per-exchange to per-component-construction,
  which shrinks rc-mgki's exposure window but does not close it.

## Alternatives considered

- Reuse the endpoint's pinned-cache machinery for the unpinned client.
  Rejected: the unpinned client has no key, no TTL, no capacity — it is one
  plain client per component, a field suffices.
- Lazy `OnceCell` client per component. Rejected: constructors are the
  natural single point; lazy adds a branch for no benefit.
- Counter on `HttpEndpoint` construction instead of `build_client`.
  Rejected: the defect is build invocations; counting them directly is the
  honest seam.
