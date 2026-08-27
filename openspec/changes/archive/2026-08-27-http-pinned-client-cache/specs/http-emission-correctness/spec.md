## ADDED Requirements

### Requirement: Pinned DNS client reuse

The `camel-http` producer SHALL reuse a cached, DNS-pinned `reqwest::Client`
for repeated requests whose SSRF-validated resolution `(host, addrs)` is
identical while the cached entry remains logically retrievable, instead of
constructing a new client per request. Per-request
SSRF resolution and validation (`resolve_initial_url_for_ssrf`) SHALL remain
in the request path unchanged. The cache SHALL be bounded by both a
time-to-live and a maximum entry count.

#### Scenario: Repeated identical resolution reuses the pinned client

- **GIVEN** a producer whose requests resolve to the same `(host, addrs)` pair
- **WHEN** two requests are sent while the cached entry remains logically
  retrievable
- **THEN** the pinned-client builder runs at most once (observed through the
  cache's build counter), and both requests use a client whose
  `resolve_to_addrs` pin equals the validated address set

#### Scenario: TTL expiry causes a rebuild

- **GIVEN** a cached pinned client for `(host, A)` whose TTL has elapsed
- **WHEN** a new request validates the same `(host, A)`
- **THEN** a fresh client is built and cached — the expired entry is
  logically dead (non-retrievable) even if its physical reclamation is
  deferred by cache maintenance

#### Scenario: Concurrent misses on the same key build once

- **GIVEN** no cached client exists for a validated `(host, addrs)` key
- **WHEN** multiple concurrent requests need that same key
- **THEN** the pinned-client builder runs once and every request uses the
  resulting client

#### Scenario: Producers of one endpoint share the cache

- **GIVEN** producers created from one endpoint share its pinned-client cache
- **WHEN** two such producers send requests with the same validated
  `(host, addrs)` while the entry remains logically retrievable
- **THEN** both hit the same cache entry and at most one client is built

#### Scenario: Changed resolution builds a fresh pinned client

- **GIVEN** a cached pinned client for `(host, A)` where `A` is one addr set
- **WHEN** a request resolves the same host to a different addr set `B`
- **THEN** a new client pinned to `(host, B)` is built and cached, and the
  request connects only to addresses in `B`

#### Scenario: Addr-set order does not split cache entries

- **GIVEN** DNS returns the same address set in different order across requests
- **WHEN** the cache key is constructed
- **THEN** the addr vector is sorted and deduplicated so both requests hit the
  same cache entry while it remains logically retrievable

#### Scenario: Cache retention is bounded

- **GIVEN** a cache entry older than the TTL, or a cache at capacity
- **WHEN** a lookup or insertion occurs after expiry or beyond capacity
- **THEN** the expired entry is no longer retrievable and the retained entry
  count remains bounded; physical reclamation of the evicted client occurs
  after deferred cache maintenance removes the cache-owned handle and the
  last external clone held by an in-flight request drops

#### Scenario: IP-literal URLs bypass the pinned cache

- **GIVEN** a request URL whose host is an IP literal
- **WHEN** `resolve_initial_url_for_ssrf` returns `None`
- **THEN** the request uses the endpoint's shared client and the pinned cache
  records no entry; the same bypass applies to redirect hops whose target
  host is an IP literal — such hops use an unpinned client and never enter
  the cache

#### Scenario: Redirect hops use the same cache

- **GIVEN** a producer request that follows a manual redirect to a new host
  that is a hostname (not an IP literal)
- **WHEN** the redirect hop resolves and validates `(redirect_host, addrs)`
- **THEN** the hop's client is obtained from the same pinned-client cache, so
  repeated redirects to the same validated target reuse one client while the
  entry remains logically retrievable
