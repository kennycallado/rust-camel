## MODIFIED Requirements

### Requirement: Sampler is parent-based

The configured sampler SHALL be wrapped in `Sampler::ParentBased`, so child spans
respect the parent sampling decision. Inside the parent-based root delegate, the
sampler SHALL first honour links (a parentless span built with links follows the
first link's sampled flag), then apply route exclusion: when the platform
configuration declares `exclude_routes` glob patterns under
`[observability.otel]`, a parentless root span whose creation attributes contain
a `route_id` matching any pattern (full match, case-sensitive) SHALL be dropped.
Non-matching roots and spans without a `route_id` attribute SHALL use the
configured root sampler unchanged. Each configured pattern SHALL compile as a
glob at configuration load; an invalid pattern SHALL fail the load with
`CamelError::Config` naming the pattern. An empty `exclude_routes` list SHALL
leave sampling behaviour identical to the unconfigured behaviour. Route
exclusion SHALL NOT change metrics recording.

#### Scenario: Unsampled parent records no child spans

- **Given** a sampler built with a ratio-based root and an exchange arriving with a valid but unsampled parent context
- **When** `should_sample` is evaluated for a child span of that parent
- **Then** the decision is `Drop`, deterministically, without any exporter or network

#### Scenario: Sampled parent keeps children

- **Given** an exchange arriving with a sampled parent context
- **When** `should_sample` is evaluated for a child span of that parent
- **Then** the decision is `RecordAndSample` regardless of the configured root ratio

#### Scenario: All configured sampler variants wrap parent-based

- **Given** each `OtelSampler` variant (`AlwaysOn`, `AlwaysOff`, `TraceIdRatioBased`)
- **When** `to_sdk_sampler` converts it
- **Then** the result is a parent-based sampler delegating to that root

#### Scenario: Excluded route root is dropped

- **Given** a sampler configured with `exclude_routes = ["skip-request*"]` and an always-on root
- **When** `should_sample` is evaluated for a parentless root whose creation attributes contain `route_id = "skip-request"`
- **Then** the decision is `Drop` and no exporter or network is involved

#### Scenario: Non-excluded route root keeps the root sampler decision

- **Given** a sampler configured with `exclude_routes = ["skip-request*"]` and an always-on root
- **When** `should_sample` is evaluated for a parentless root whose creation attributes contain `route_id = "orders-in"`
- **Then** the decision is `RecordAndSample`

#### Scenario: Sampled parent wins over exclusion

- **Given** a sampler whose `exclude_routes` matches the route and an exchange arriving with a sampled parent context
- **When** `should_sample` is evaluated for a child span of that route
- **Then** the decision is `RecordAndSample` (parent-based semantics are preserved)

#### Scenario: Linked root follows its origin's sampling decision

- **Given** a sampler whose `exclude_routes` matches the route and a parentless root built with a link to an unsampled origin span context
- **When** `should_sample` is evaluated for that linked root
- **Then** the decision is `Drop` (the link's sampled flag governs before exclusion is consulted)

#### Scenario: Invalid pattern fails configuration load

- **Given** a platform configuration declaring `[observability.otel] exclude_routes = ["[unclosed"]`
- **When** the configuration is loaded
- **Then** the load fails with `CamelError::Config` and the message names the invalid pattern

#### Scenario: Empty exclusion list preserves behaviour

- **Given** a platform configuration with `exclude_routes` absent or empty
- **When** roots are sampled for any route id
- **Then** every decision equals the decision of the configured root sampler alone

#### Scenario: Excluded route still records metrics

- **Given** an excluded route executing exchanges with a metrics collector attached
- **When** the exchanges complete
- **Then** exchange-count and duration metrics for that route are recorded exactly as for a non-excluded route
