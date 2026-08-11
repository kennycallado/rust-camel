## ADDED Requirements

### Requirement: Component metadata coverage

Components that accept meaningful URI query parameters SHALL expose a private
metadata-descriptor struct per scheme, carrying only `#[uri_param]` fields, with
`#[derive(UriConfig)]` + `#[uri_config(skip_impl, metadata(scheme = "..", ..))]` +
`#[uri_scheme = ".."]` on the descriptor. The Component's `metadata()` SHALL delegate
to the descriptor's inherent `metadata()`. This descriptor pattern is universal in this
change because production runtime config structs carry multiple non-`#[uri_param]`
fields (path-derived names, resolved values, injected handles) that exceed the macro's
single non-param "path" field limit (`uri_config.rs:856-865`), making direct derives on
the runtime config fail to compile. After the opt-in,
`ComponentMetadataCatalog::get_metadata` for the scheme SHALL return a
`ComponentMetadata` whose `uri_options` is non-empty and whose entry names/aliases match
the Component's accepted query keys. The Component's existing manual `from_uri` parsing
and runtime config struct SHALL remain unchanged.

#### Scenario: kafka exposes its broker/group params

- **GIVEN** a `KafkaMetadataDescriptor` carries `#[uri_param]` for `brokers`, `groupId`, and `autoOffsetReset`
- **WHEN** `get_metadata("kafka")` is harvested at registration
- **THEN** `uri_options` contains entries whose canonical name or alias is `brokers`, `groupId`, and `autoOffsetReset` (non-empty)

#### Scenario: mqtt exposes qos and clientId

- **GIVEN** an `MqttMetadataDescriptor` carries `#[uri_param]` for `qos` and `clientId`
- **WHEN** `get_metadata("mqtt")` is called
- **THEN** `uri_options` contains entries whose canonical name or alias is `qos` and `clientId`

#### Scenario: redis exposes command and key

- **GIVEN** a `RedisMetadataDescriptor` carries `#[uri_param]` for `command` and `key`
- **WHEN** `get_metadata("redis")` is called
- **THEN** `uri_options` contains entries whose canonical name or alias is `command` and `key`

#### Scenario: controlbus exposes its security-critical params

- **GIVEN** the controlbus Component derives `UriConfig` (`skip_impl`) with `#[uri_param]` for `routeId`, `action`, and `authorizedRoutes`
- **WHEN** `get_metadata("controlbus")` is called
- **THEN** `uri_options` contains entries whose canonical name or alias is `routeId`, `action`, and `authorizedRoutes`

### Requirement: Executable parser/metadata parity

For each Component annotated under "Component metadata coverage", the change SHALL add a
per-Component test that asserts the metadata canonical-name/alias set equals a reviewed
fixture of the keys the Component's `from_uri` parser accepts. This makes
parser/metadata agreement an executable invariant rather than a manual one: the test
fails if the parser grows a key the metadata omits (lint would false-positive) or if the
metadata names a key the parser rejects (metadata would mislead).

#### Scenario: metadata names exactly match parser keys

- **GIVEN** a Component whose `from_uri` accepts keys `{A, B, C}` and declares `#[uri_param]` for `A`, `B`, and `C` (by canonical name or alias)
- **WHEN** the parity test runs `get_metadata(<scheme>).uri_options` names/aliases against the fixture `{A, B, C}`
- **THEN** the two sets are equal (no missing, no extra)

#### Scenario: secret/required/default flags track parser semantics

- **GIVEN** a Component whose parser treats key `password` as a secret (never logged), key `brokers` as required, and key `qos` with parser default `0`
- **WHEN** the `#[uri_param]` metadata is authored for those fields
- **THEN** `password` carries `secret`, `brokers` carries `required`, and `qos` carries `default = "0"` (the metadata default equals the parser's default), so downstream tooling classifies them consistently with the parser

#### Scenario: parsing is unchanged by annotation

- **GIVEN** a Component whose config struct gained `#[derive(UriConfig)]` + `#[uri_param]` (`skip_impl`)
- **WHEN** its `from_uri("scheme:path?param=value")` is called
- **THEN** the returned config parses identically to before the annotation (same fields populated, same errors for bad input) — `skip_impl` guarantees the derive added no trait impl that could alter parsing

### Requirement: Metadata-descriptor struct when the runtime config cannot derive directly

When a Component's runtime config struct carries more than one non-`#[uri_param]`
field — path-derived names, resolved/default values, injected handles (auth token
providers, HTTP clients), connection state, or any other non-URI field — it exceeds the
macro's single non-`#[uri_param]` "path" field limit and cannot derive `UriConfig`
directly. The change SHALL instead author a dedicated private metadata-descriptor
struct containing only the `#[uri_param]` URI fields and derive `UriConfig` on that
descriptor. The Component's `metadata()` delegates to the descriptor's inherent
`metadata()`. The runtime config struct is not modified to derive, and its `from_uri`
parsing is unchanged.

#### Scenario: keycloak endpoint configs use a descriptor struct

- **GIVEN** keycloak's `AdminEndpointConfig` and `EventsEndpointConfig` carry injected `token_provider: Arc<dyn TokenProvider>` and `http: reqwest::Client` fields that cannot be `#[uri_param]`-annotated (and `KeycloakRealmConfig` holds only non-URI primitives, not the endpoint params)
- **WHEN** the change authors a private keycloak metadata-descriptor struct with the endpoint URI params (`target_realm`, `operation`, `user_id`, …)
- **THEN** `get_metadata("keycloak")` returns the descriptor's `uri_options` (non-empty), and `AdminEndpointConfig`/`EventsEndpointConfig`/`KeycloakRealmConfig` are unchanged in shape and parsing

### Requirement: Per-Component disposition for query-minimal and namespace-blocked

For Components that are NOT annotated (query-minimal or open-namespace-blocked), the
change SHALL record an explicit per-Component disposition in its Phase-2 task:
`advisory` (legitimately query-minimal — `minimal(scheme)` is correct, no work) or
`schema-blocked-deferred` (accepts an open-ended `param.*` namespace that exact
`UriOption` names cannot model — deferred until the macro/catalog support open-ended
namespaces).

#### Scenario: exec recorded as advisory

- **GIVEN** the exec Component is profile-driven and ignores URI query strings
- **WHEN** its Phase-2 disposition is recorded
- **THEN** it is marked `advisory` with the reason "profile-driven; query strings ignored", and no `#[uri_param]` is authored

#### Scenario: xj/xslt recorded as schema-blocked-deferred

- **GIVEN** the xj and xslt Components accept an open-ended `param.*` key namespace
- **WHEN** their Phase-2 disposition is recorded
- **THEN** they are marked `schema-blocked-deferred` with the reason "open-ended param.* namespace unsupported by exact UriOption names", and a follow-up is noted for macro/catalog open-namespace support
