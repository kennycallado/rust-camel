## ADDED Requirements

### Requirement: Component-scoped shared unpinned client

The `camel-http` HTTP and HTTPS components SHALL construct one shared
unpinned `reqwest::Client` per component instance and SHALL pass clones of
it to every endpoint the component creates, including endpoints created
for dynamic EIP URI resolution. Endpoint creation SHALL NOT invoke the
client builder. SSRF validation and DNS pinning semantics SHALL remain
unchanged.

#### Scenario: One component constructor builds one client

- **GIVEN** the client-builder invocation counter at a baseline
- **WHEN** one `HttpComponent` or `HttpsComponent` is constructed
- **THEN** the counter increases by exactly one

#### Scenario: Endpoint creation adds no client builds

- **GIVEN** a constructed component
- **WHEN** `create_endpoint` runs for two distinct URIs
- **THEN** the counter does not increase, and both endpoints hold clones
  of the component's single client (one shared connection pool)

#### Scenario: Dynamic EIP resolutions add no client builds

- **GIVEN** a constructed component and a counter baseline
- **WHEN** a resolution loop calls `create_endpoint` plus
  `create_producer` three times over distinct URIs, the resolution shape
  of `recipientList`/`routingSlip`/`dynamicRouter`, without sending
  requests
- **THEN** the counter does not increase

#### Scenario: Reuse preserves config identity

- **GIVEN** endpoints of one component whose unpinned client derives
  solely from the component's `HttpConfig`
- **WHEN** a request from any endpoint of that component uses the shared
  unpinned client (IP-literal URL or IP-literal redirect hop)
- **THEN** the client is built from an identical `HttpConfig`, so reuse is
  semantically equal to a rebuild
