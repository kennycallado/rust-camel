## MODIFIED Requirements

### Requirement: LintRoute captures every URI-bearing location with spans

The engine SHALL construct `LintRoute` by walking the CST (or using noyalib's span-preserving
deserialization) and SHALL capture, each with a byte-exact `Spanned<T>`: the route-level
`from` URI; each endpoint URI (`to` / `uri` leaves); and every URI option key and value
(parsed out of the URI query string, the step option map, or the endpoint `parameters` map —
`parameters:` entries SHALL be captured as options attached to the same endpoint, each key
and value with its own byte-exact span into the source). Structural containers that hold
children but carry no URI themselves (`choice` with `when`/`otherwise` branches, `multicast`,
`scatter_gather.endpoints` — the containers present in `route-schema.json`; `pipeline` does
not exist in the schema) SHALL be traversed recursively so that endpoint URIs nested at any
depth are captured. The traversal SHALL be driven by the schema: lint resolves which node
types may contain `to`/`from`/`uri` or nested children by reading the `route-schema.json`
definition, so adding a new step container in the schema requires no lint code change beyond
re-syncing the embedded copy. Capturing a location SHALL NOT require `camel-dsl`.

Each captured option SHALL carry its source origin: `Query` for an option parsed out of the
URI query string, `StepParameters` for an entry of a `parameters:` map sibling of a
URI-bearing key (including the route-level `from`), or `ConfigParameters` for an entry of
the `parameters:` map inside an object-form URI key. Origins SHALL be distinguishable by
rules, while every option remains attached to the same endpoint and is validated
identically by the per-occurrence rules (unknown-option, kind-mismatch, secret,
deprecated).

#### Scenario: Route-level from URI is captured with a span

- **GIVEN** a route file with `from: direct:start` where `direct:start` starts at byte offset 12
- **WHEN** the engine builds `LintRoute`
- **THEN** the captured `from` value has a span whose start offset is 12

#### Scenario: Nested child step URIs are captured with spans

- **GIVEN** a route file with a `choice`/`when` branch (or a `multicast`) containing a child step `to: log:nested`
- **WHEN** the engine builds `LintRoute`
- **THEN** the child step's `to` value is present in the captured steps with its own byte-exact span, distinct from the parent step's span

#### Scenario: scatter_gather endpoint URIs are captured with spans

- **GIVEN** a route file with a `scatter_gather` step whose `endpoints` array contains `direct:a` and `direct:b`
- **WHEN** the engine builds `LintRoute`
- **THEN** both endpoint URIs are captured as URI-bearing locations, each with its own byte-exact span

#### Scenario: Option keys and values are captured with spans

- **GIVEN** a step `timer:foo?period=1s` where `period` starts at byte offset 30 and `1s` at 37
- **WHEN** the engine builds `LintRoute`
- **THEN** the captured option key `period` has start offset 30, the option value `1s` has start offset 37, and the option's origin is `Query`

#### Scenario: parameters map entries are captured as options with spans

- **GIVEN** a step with `to: kafka:orders` and a `parameters:` map entry `brokers: my-host:9092` where the key starts at byte offset 42 and the value at 51
- **WHEN** the engine builds `LintRoute`
- **THEN** the captured endpoint carries an option `brokers` with key span start 42 and value span start 51, and the option's origin is `StepParameters`

#### Scenario: Step-level and inner parameters both reach a nested endpoint

- **GIVEN** a step with object-form `enrich: {uri: db:query, parameters: {dataSource: customers}}` and a sibling step-level `parameters: {timeoutS: "5000"}`
- **WHEN** the engine builds `LintRoute`
- **THEN** the nested `db:query` endpoint carries BOTH options `dataSource` and `timeoutS` — the two maps are concatenated, never either/or, so no entry is dropped from rule evaluation — and `dataSource` carries origin `ConfigParameters` while `timeoutS` carries origin `StepParameters`

#### Scenario: Route-level from parameters are captured as options

- **GIVEN** a route with `from: timer:tick` and a route-level `parameters: {period: "2500"}`
- **WHEN** the engine builds `LintRoute`
- **THEN** the `from` endpoint carries the option `period` with byte-exact key and value spans and origin `StepParameters`

## ADDED Requirements

### Requirement: R-URI-known flags cross-source duplicate option keys

For each endpoint, R-URI-known SHALL flag any option key — compared as the raw key string,
without alias resolution — that appears in more than one source origin: the URI query
string, step-level `parameters:`, or object-form config `parameters:`. This mirrors the DSL
lowering's fail-closed duplicate-key behavior (`EndpointUriError::DuplicateKey` from
query/parameters overlap and from config/step parameters overlap). The diagnostic SHALL
have code `R-URI-known:duplicate-key`, severity error, and a byte-exact span on the
redundant occurrence: the parameters-side key occurrence. Repeated keys within the raw
query string alone SHALL NOT be flagged (the lowering preserves them in order). The check
SHALL run independently of catalog knowledge: an unregistered scheme SHALL still be
flagged. At most one duplicate-key diagnostic SHALL be emitted per colliding key per
endpoint, even when the key appears in all three sources.

#### Scenario: Query string plus sibling parameters is flagged

- **GIVEN** a step `to: timer:foo?period=1000` with a sibling `parameters: {period: "2500"}` and a catalog that knows `timer` with option `period`
- **WHEN** R-URI-known runs
- **THEN** exactly one `R-URI-known:duplicate-key` error is emitted with a span on the `period` key inside the `parameters:` map

#### Scenario: Config parameters plus step parameters is flagged

- **GIVEN** a step with object-form `enrich: {uri: db:query, parameters: {timeout: "1"}}` and a sibling step-level `parameters: {timeout: "2"}`
- **WHEN** R-URI-known runs
- **THEN** exactly one `R-URI-known:duplicate-key` error is emitted with a span on the `timeout` key inside the step-level `parameters:` map

#### Scenario: Repeated query keys alone are not flagged

- **GIVEN** a step `to: timer:foo?period=1&period=2` with no `parameters:` map
- **WHEN** R-URI-known runs
- **THEN** no `R-URI-known:duplicate-key` diagnostic is emitted

#### Scenario: Unregistered scheme still flagged

- **GIVEN** a step `to: kafka:orders?brokers=h1` with a sibling `parameters: {brokers: "h2"}` and a catalog with no entry for `kafka`
- **WHEN** R-URI-known runs
- **THEN** a `R-URI-known:duplicate-key` error is emitted with a span on the `brokers` key inside the `parameters:` map, in addition to the informational `unverified-scheme` note

#### Scenario: Route-level from overlap is flagged

- **GIVEN** a route with `from: timer:tick?period=1s` and a route-level `parameters: {period: "2500"}`
- **WHEN** R-URI-known runs
- **THEN** a `R-URI-known:duplicate-key` error is emitted with a span on the `period` key inside the route-level `parameters:` map

#### Scenario: Key in all three sources yields one diagnostic

- **GIVEN** a step with object-form `to: {uri: timer:foo?period=1s, parameters: {period: "2"}}` and a sibling step-level `parameters: {period: "3"}`
- **WHEN** R-URI-known runs
- **THEN** exactly one `R-URI-known:duplicate-key` diagnostic is emitted for the key `period` on that endpoint
