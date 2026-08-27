# Design: http-shared-pinned-cache

## Approach

Hoist the `PinnedClientCache` from endpoint construction time to component
construction time.

1. `HttpComponent` and `HttpsComponent` gain
   `pinned_cache: Arc<PinnedClientCache>`, initialized in their constructors
   (`new`, and any `Default` path that funnels through `new`). The constants
   stay `PINNED_CLIENT_TTL` and `PINNED_CLIENT_MAX_ENTRIES`.
2. `create_endpoint` (`lib.rs:1893` http, `lib.rs:1963` https) drops its
   `PinnedClientCache::new` call and clones the component's `Arc` into
   `HttpEndpoint`. The `HttpEndpoint` and `HttpProducer` field types do not
   change. Existing plumbing (`Arc::clone` into producers, redirect path)
   stays as is.
3. No `camel-core` change. `make_endpoint_resolver` resolves the active
   registered component `Arc` per URI
   (`CamelContext::resolve_component`, `context.rs:943`). The registry
   stores one active component per scheme, and `Registry::register`
   (`registry.rs:44`) can replace it. Sharing scope is therefore the
   component instance, not the scheme: all endpoints and dynamic
   resolutions served by one instance share its cache, a replacement
   component starts a separate cache, and existing endpoints keep the cache
   they were built with.

Sharing-scope invariant: the cache key is `(host, canonicalized addrs)`.
Client construction closes over the endpoint's `http_config`. That field is
a clone of the component's `HttpConfig` at `create_endpoint` time
(`lib.rs:1916`, `lib.rs:1986`). Distinct endpoints of one component thus
build config-identical clients, so a cache hit across endpoints is
semantically equal to a rebuild. `HttpComponent` and `HttpsComponent`
remain separate component instances with separate caches.

Tests reuse the existing `build_count()` seam. New unit tests create one
component, call `create_endpoint` twice with distinct URIs, create
producers, and send to the same local responder. Assert one build. One test
simulates the dynamic path by looping `create_endpoint` plus
`create_producer` over distinct URIs (distinct paths, same host). The
existing shared-cache helper tests around `lib.rs:7715+` inject caches
through `endpoint_with_shared_cache` and stay valid. After adding the
field, grep for internal struct literals of the two component structs.

Docs: `crates/components/camel-http/CONTEXT.md` gains a short operator note
in the client-reuse section. Shared cache footprint: up to 64 cached
clients, each able to hold up to `pool_max_idle_per_host` (default 100)
idle connections per distinct host, released after `pool_idle_timeout`
(default 90 s).

## Affected crates

- camel-http: component structs, `create_endpoint`, unit tests, CONTEXT.md.

## Architecture boundaries

Components layer only. No Runtime, DSL, Services, Languages, or Functions
change. `camel-core` keeps its resolver as is. The component stays the
owner of client lifecycle. No public API change: `PinnedClientCache` stays
`pub(crate)`, struct fields stay private.

## Out of scope, observed but not changed

- `create_endpoint` calls `register_current_route_health_check` per
  invocation. Dynamic resolutions call it per exchange. Components across
  the repo (cxf, opensearch, sql, xslt) share this pattern. Registry
  semantics at runtime resolution are not audited here.
- Dynamic paths still build one shared unpinned `reqwest::Client` per
  `create_endpoint` call. Pre-existing behavior, one client per resolution
  rather than per request.
- PEM re-read cadence note (bd rc-mgki).

## Alternatives considered

- Memoize endpoints in `make_endpoint_resolver`. Generic, but a
  `camel-core` change for all components that needs its own eviction
  policy. Rejected for this change.
- Process-wide static cache. Crosses contexts, hurts test isolation, and
  has unbounded lifetime. Rejected.
- Per-route cache inside the resolver closure. The resolver is one shared
  closure per compiled step, so this still needs a URI-keyed map and
  duplicates the component's ownership of client lifecycle. Rejected.
