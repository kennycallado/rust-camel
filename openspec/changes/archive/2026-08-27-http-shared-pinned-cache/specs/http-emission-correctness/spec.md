## ADDED Requirements

### Requirement: Component-scoped pinned client cache

The `camel-http` HTTP and HTTPS components SHALL construct one
pinned-client cache per component instance and SHALL share it across every
endpoint the component creates, including endpoints created for dynamic EIP
URI resolution (`recipientList`, `routingSlip`, `dynamicRouter`). Per-request
SSRF resolution and validation SHALL remain unchanged.

#### Scenario: Endpoints of one component share one cache

- **GIVEN** one `HttpComponent` and two endpoints created from it through
  distinct URIs
- **WHEN** producers of both endpoints send requests that validate to the
  same `(host, addrs)` while the entry remains logically retrievable
- **THEN** the pinned-client builder runs at most once and both producers
  use the same cached client

#### Scenario: Dynamic EIP resolutions hit the shared cache

- **GIVEN** a component whose `create_endpoint` is called repeatedly for
  distinct runtime URIs that share one host, the resolution shape of
  `recipientList`/`routingSlip`/`dynamicRouter`
- **WHEN** each resolution creates a producer and sends a request whose
  validated `(host, addrs)` is identical
- **THEN** the pinned-client builder runs at most once across the
  resolutions while the entry remains logically retrievable

#### Scenario: HTTPS component owns its own cache

- **GIVEN** separate `HttpComponent` and `HttpsComponent` instances
- **WHEN** each creates an endpoint
- **THEN** each endpoint's pinned cache is the one owned by its component,
  and the two caches are distinct

#### Scenario: Reuse across endpoints preserves config identity

- **GIVEN** endpoints of one component whose `http_config` is a clone of
  the component's `HttpConfig`
- **WHEN** a cache hit serves a request from a different endpoint of the
  same component
- **THEN** the reused client is built from an identical `HttpConfig`, so
  reuse is semantically equal to a rebuild
