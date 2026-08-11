# endpoint-metadata-derivation Specification

## Purpose
TBD - created by archiving change consolidate-uri-metadata. Update Purpose after archive.
## Requirements
### Requirement: Macro-derived URI options

The `#[derive(UriConfig)]` macro SHALL generate a `fn uri_options() -> Vec<UriOption>`
helper on every derived struct, built from the struct's `#[uri_param]`-annotated fields.

#### Scenario: uri_options populated from uri_param fields

- **GIVEN** a struct derives `UriConfig` with two `#[uri_param]` fields
- **WHEN** `Self::uri_options()` is called
- **THEN** it returns two `UriOption` entries whose names match the `#[uri_param]` names

#### Scenario: path field is excluded from uri_options

- **GIVEN** a struct with one non-`#[uri_param]` path field and one `#[uri_param]` field
- **WHEN** `Self::uri_options()` is called
- **THEN** only the `#[uri_param]` field appears in the result (the path field is omitted)

### Requirement: OptionKind type inference

The macro SHALL infer `OptionKind` from the Rust field type (after unwrapping
`Option<T>`) and SHALL NOT infer `OptionKind::Enum` for any type. `Enum` is producible
only via an explicit `kind` attribute override.

#### Scenario: primitive type inference

- **GIVEN** a `#[uri_param]` field of type `bool`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::Bool`

#### Scenario: Duration type inference

- **GIVEN** a `#[uri_param]` field of type `Duration`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::Duration`

#### Scenario: enum-typed field infers String not Enum

- **GIVEN** a `#[uri_param]` field of an enum type with a `FromStr` impl and no explicit `kind`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::String` (never `Enum`)

#### Scenario: explicit kind override to Enum

- **GIVEN** a `#[uri_param(kind = "enum:A,B")]` field
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::Enum` with variants `["A", "B"]`

### Requirement: Semantic uri_param keys

The macro SHALL accept `desc`, `required`, `secret`, `deprecated`, `aliases`, and `kind`
keys on `#[uri_param]`, mapping each to the corresponding `UriOption` field. Truly
unknown keys SHALL remain a compile error.

#### Scenario: secret key sets the secret flag

- **GIVEN** a `#[uri_param(secret)]` field
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `secret` flag is `true`

#### Scenario: deprecated key records the reason

- **GIVEN** a `#[uri_param(deprecated = "use newX instead")]` field
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `deprecated` is `Some("use newX instead")`

#### Scenario: aliases key populates the alias list

- **GIVEN** a `#[uri_param(aliases = ["oldName", "legacyName"])]` field
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `aliases` contains `"oldName"` and `"legacyName"`

#### Scenario: unknown key remains an error

- **GIVEN** a `#[uri_param(unknwonKey = "value")]` field
- **WHEN** the macro is expanded
- **THEN** compilation fails with an "unknown attribute key" error

### Requirement: Secret with default is rejected

The macro SHALL emit a compile error when a `#[uri_param]` field is both `secret` and
carries a non-empty `default`, preventing hardcoded secret leakage into metadata.

#### Scenario: secret and default together fails compilation

- **GIVEN** a `#[uri_param(secret, default = "hunter2")]` field
- **WHEN** the macro is expanded
- **THEN** compilation fails with an error indicating a secret must not have a default

### Requirement: Opt-in metadata generation

The macro SHALL generate a `Component::metadata()` override when the struct carries a
`#[uri_config(metadata(scheme = "..", description = "..", ..))]` attribute, populating
`ComponentMetadata.uri_options` from the derived `uri_options()`. Components without the
opt-in SHALL retain the default `minimal(scheme)` behavior.

#### Scenario: opt-in metadata populates uri_options

- **GIVEN** a struct with `#[uri_config(metadata(scheme = "sql", description = "..", producer))]` and two `#[uri_param]` fields
- **WHEN** the component's `metadata()` is harvested at registration
- **THEN** `get_metadata("sql")` returns `uri_options` with two entries

#### Scenario: no opt-in retains minimal metadata

- **GIVEN** a struct that derives `UriConfig` but has no `#[uri_config(metadata(..))]`
- **WHEN** the component's `metadata()` is called
- **THEN** it returns `ComponentMetadata::minimal(scheme)` with empty `uri_options` (unchanged behavior)

### Requirement: Component-to-config metadata delegation

Because `#[derive(UriConfig)]` derives on the *config* struct (a different type than the
*component* that implements `Component`), each migrated component's `Component::metadata()`
override SHALL delegate to the config struct's inherent `metadata()` (or compose via
`ComponentMetadata::minimal(scheme).with_uri_options(ConfigType::uri_options())`).

#### Scenario: component delegates to config metadata

- **GIVEN** a component whose config struct derives UriConfig with `#[uri_config(metadata(..))]`
- **WHEN** the component is registered and `get_metadata(scheme)` is queried
- **THEN** the returned metadata has non-empty `uri_options` sourced from the config's derived `uri_options()`

### Requirement: ComponentMetadata builders

`ComponentMetadata` SHALL provide builder methods `with_description`, `with_capabilities`,
and `with_uri_options` so that generated and hand-written metadata can compose uniformly.
These methods are currently absent (`ComponentMetadata` has only `minimal()`) and SHALL be
added unconditionally in Phase 1.

#### Scenario: with_uri_options appends options

- **GIVEN** a `ComponentMetadata::minimal("sql")` with empty options
- **WHEN** `.with_uri_options(opts)` is called with a non-empty `Vec<UriOption>`
- **THEN** the resulting metadata contains those options in `uri_options`

#### Scenario: with_capabilities sets capability flags

- **GIVEN** a `#[uri_config(metadata(scheme = "x", producer, consumer))]` attribute
- **WHEN** the generated `metadata()` is harvested at registration
- **THEN** the resulting `capabilities.producer == true` and `capabilities.consumer == true`

### Requirement: Kind override validation

An unrecognized `kind` string SHALL be a compile error with a spanned message, not a
silent fallthrough to `String`.

#### Scenario: kind typo is a compile error

- **GIVEN** a `#[uri_param(kind = "duraton")]` field (misspelled)
- **WHEN** the macro is expanded
- **THEN** compilation fails with an error naming the unrecognized kind

### Requirement: Required-flag coherence with Option types

A field typed `Option<T>` SHALL NOT emit `required = true` unless the `required` key is
explicitly present on the `#[uri_param]`.

#### Scenario: Option field without required attr

- **GIVEN** a `password: Option<String>` field with `#[uri_param]` and no `required` key
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `required` flag is `false`

### Requirement: Single source of truth for all component metadata

The project SHALL NOT retain hand-written `fn metadata()` UriOption lists in any
component crate. Every component's parameter metadata SHALL derive from the
`#[derive(UriConfig)]` macro via `#[uri_param]` field annotations, so that parsing and
catalog metadata have exactly one authoring source.

#### Scenario: no hand-written UriOption lists remain

- **GIVEN** the workspace after all migrations
- **WHEN** non-test source in component crates is searched for `UriOption::new` calls
- **THEN** zero matches are found outside `camel-api` (definition site) and
  `camel-endpoint-macros` (generation site); `#[cfg(test)]` modules are excluded

#### Scenario: no duplicate option names per scheme

- **GIVEN** any scheme in the catalog
- **WHEN** its `uri_options` are inspected
- **THEN** no two options share the same `name`

### Requirement: List inference for Vec types

The macro SHALL infer `OptionKind::List` for `Vec<T>` fields, with the inner kind
derived from `T`.

#### Scenario: Vec of String infers List of String

- **GIVEN** a `#[uri_param]` field of type `Vec<String>`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::List` with inner kind `String`

### Requirement: Catalog visibility for all macro-derived components

Every component that derives `UriConfig` and opts into metadata generation SHALL have its
parameters visible through the `ComponentMetadataCatalog`, so downstream lint and tooling
do not report valid parameters as unknown.

#### Scenario: sql parameters visible in catalog

- **GIVEN** the sql component has opted into `#[uri_config(metadata(..))]`
- **WHEN** `catalog.get_metadata("sql")` is queried
- **THEN** the returned `uri_options` is non-empty and contains the params declared via `#[uri_param]`

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

