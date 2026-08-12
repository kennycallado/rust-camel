# endpoint-metadata-derivation Specification

## Purpose
TBD - created by archiving change consolidate-uri-metadata. Update Purpose after archive.
## Requirements
### Requirement: Macro-derived URI options

The `#[derive(UriConfig)]` macro SHALL generate a `fn uri_options() -> Vec<UriOption>`
helper on every derived struct, built from the struct's `#[uri_param]`-annotated fields.

For a field annotated with `#[uri_param(pattern = "<separator>")]`, the
separator SHALL end with the character `.` (this is the only permitted
separator shape in this version; future `UriOptionMatch` variants may relax
this). The generated `UriOption` `name` SHALL equal the separator with the
trailing `.` removed (so separator `"param."` → `name = "param"`; the
algorithm is total because the trailing-`.` precondition is enforced at
compile time). For all other `#[uri_param]` fields, the generated `UriOption`
`name` SHALL match the existing rule (`#[uri_param(name = "..")]` override,
else the Rust field name snake_cased).

#### Scenario: uri_options populated from uri_param fields

- **GIVEN** a struct derives `UriConfig` with two `#[uri_param]` fields without `pattern`
- **WHEN** `Self::uri_options()` is called
- **THEN** it returns two `UriOption` entries whose names match the `#[uri_param]` names (existing behavior, unchanged)

#### Scenario: pattern field name is derived from the separator

- **GIVEN** a struct with a `#[uri_param(pattern = "param.")]` field of type `Vec<(String, String)>`
- **WHEN** `Self::uri_options()` is called
- **THEN** the resulting entry has `name = "param"` (separator with trailing suffix stripped), not the Rust field's name

#### Scenario: path field is excluded from uri_options

- **GIVEN** a struct with one non-`#[uri_param]` path field and one `#[uri_param]` field
- **WHEN** `Self::uri_options()` is called
- **THEN** only the `#[uri_param]` field appears in the result (the path field is omitted)

### Requirement: OptionKind type inference

The macro SHALL infer `OptionKind` from the Rust field type (after unwrapping
`Option<T>`) and SHALL NOT infer `OptionKind::Enum` for any type. `Enum` is producible
only via an explicit `kind` attribute override.

For a `#[uri_param(pattern = "..")]` field, the inferred `OptionKind` SHALL be `String`
(regardless of the Rust type, which is constrained to `Vec<(String, String)>` by the
guardrails in the "Semantic uri_param keys" requirement). An explicit `kind` key is
permitted on a pattern field ONLY with the value `"string"` (or omitted); any other
`kind` value on a pattern field SHALL be a compile error.

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

#### Scenario: pattern field infers String regardless of Vec rust type

- **GIVEN** a `#[uri_param(pattern = "param.")]` field of type `Vec<(String, String)>`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::String` (NOT `OptionKind::List`, even though the Rust type is `Vec`)

#### Scenario: non-string kind on a pattern field fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.", kind = "duration")]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `kind` on a pattern field must be `string` or omitted

### Requirement: Semantic uri_param keys

The macro SHALL accept `name`, `default`, `desc`, `required`, `secret`, `deprecated`,
`aliases`, `kind`, and `pattern` keys on `#[uri_param]`, mapping each to the
corresponding `UriOption` field. Truly unknown keys SHALL remain a compile error.

The `pattern = "<separator>"` key SHALL be valid only on fields of type
`Vec<(String, String)>`. The macro SHALL reject the following combinations at
compile time with a spanned diagnostic:

- `pattern` together with `required` (an open namespace cannot require a single key).
- `pattern` together with `default` (an open namespace has no default value).
- `pattern` together with `secret` (an open namespace has no single secret value).
- `pattern` together with `name` (the name is derived from the separator; explicit
  override is forbidden).
- `pattern` together with `aliases` (a namespace matches by prefix; aliases are
  exact-match and would contradict the prefix rule).
- `pattern` together with a non-`string` `kind` (per "OptionKind type inference").
- `pattern = ""` (an empty separator would match every key, defeating the lint).
- `pattern` whose value does not end with `.` (the only permitted separator
  shape in this version; the name derivation algorithm relies on this
  precondition — see "Macro-derived URI options").

When `pattern` is present, the generated `UriOption` SHALL have
`kind = OptionKind::String` and `name` derived per the "Macro-derived URI options"
requirement (separator with trailing `.` removed). The macro SHALL emit the
option via a new consuming builder
`UriOption::pattern_prefix(separator: &str) -> Self` that sets
`pattern: Some(UriOptionMatch::Prefix { separator: separator.to_string() })`.

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

#### Scenario: pattern key on a Vec of pairs produces a namespace option

- **GIVEN** a `#[uri_param(pattern = "param.")]` field of type `Vec<(String, String)>`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option has `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`, `name = "param"`, and `kind = OptionKind::String`

#### Scenario: pattern key on a String field fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.")]` field of type `String`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` is only valid on `Vec<(String, String)>`

#### Scenario: pattern key on a Vec of String fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.")]` field of type `Vec<String>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` is only valid on `Vec<(String, String)>` (the field type is the wrong shape even though it is a `Vec`)

#### Scenario: pattern with required fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.", required)]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` and `required` are incompatible

#### Scenario: pattern with default fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.", default = "x")]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` and `default` are incompatible

#### Scenario: pattern with secret fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.", secret)]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` and `secret` are incompatible

#### Scenario: pattern with name override fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.", name = "stylesheetParams")]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` and `name` are incompatible (the name is derived from the separator)

#### Scenario: pattern with aliases fails compilation

- **GIVEN** a `#[uri_param(pattern = "param.", aliases = ["x"])]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating `pattern` and `aliases` are incompatible

#### Scenario: empty pattern separator fails compilation

- **GIVEN** a `#[uri_param(pattern = "")]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating the separator must be non-empty

#### Scenario: pattern separator without trailing dot fails compilation

- **GIVEN** a `#[uri_param(pattern = "param/")]` field of type `Vec<(String, String)>`
- **WHEN** the macro is expanded
- **THEN** compilation fails with a spanned error indicating the separator must end with `.` (the only permitted separator shape in this version; future `UriOptionMatch` variants may relax this)

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

This inference SHALL NOT apply to fields annotated with `#[uri_param(pattern = "..")]`,
which are constrained to the Rust type `Vec<(String, String)>` and SHALL infer
`OptionKind::String` per the "OptionKind type inference" requirement (the namespace's
values are string key/value pairs, not a list of scalars).

#### Scenario: Vec of String infers List of String

- **GIVEN** a `#[uri_param]` field of type `Vec<String>` without `pattern`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::List` with inner kind `String`

#### Scenario: Vec of (String, String) without pattern infers List

- **GIVEN** a `#[uri_param]` field of type `Vec<(String, String)>` without `pattern`
- **WHEN** the macro generates the `UriOption`
- **THEN** the option `kind` is `OptionKind::List` with inner kind `String` (the pair-tuple inner kind falls back to String; existing behavior)

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

Each change that touches URI metadata SHALL record an explicit per-Component
disposition for every component in its Phase-2 task. The disposition reflects
the component's current state and may transition across changes:
`advisory` (legitimately query-minimal — `minimal(scheme)` is correct, no work),
`schema-blocked-deferred` (accepts an open-ended `param.*` namespace that exact
`UriOption` names cannot model — deferred until the macro/catalog support open-ended
namespaces), or `schema-published` (the open namespace is declared via a
`pattern`-based `#[uri_param]` and rich metadata is published through a
`skip_impl` descriptor with a `Component::metadata()` override). A component
may transition from `schema-blocked-deferred` to `schema-published` once
open-namespace macro support is available.

#### Scenario: exec recorded as advisory

- **GIVEN** the exec Component is profile-driven and ignores URI query strings
- **WHEN** its Phase-2 disposition is recorded
- **THEN** it is marked `advisory` with the reason "profile-driven; query strings ignored", and no `#[uri_param]` is authored

#### Scenario: xj/xslt recorded as schema-blocked-deferred

- **GIVEN** the xj and xslt Components accept an open-ended `param.*` key namespace
- **WHEN** their Phase-2 disposition is recorded in a change that predates open-namespace macro support
- **THEN** they are marked `schema-blocked-deferred` with the reason "open-ended param.* namespace unsupported by exact UriOption names", and a follow-up is noted for macro/catalog open-namespace support

#### Scenario: xj/xslt transitioned to schema-published

- **GIVEN** the xj and xslt Components were previously marked `schema-blocked-deferred`
- **WHEN** a change adds a `skip_impl` metadata descriptor with a `#[uri_param(pattern = "param.")]` field and a `Component::metadata()` override to each component
- **THEN** their disposition transitions to `schema-published`, the catalog returns non-empty `uri_options` for schemes `xj` and `xslt`, and the `param` option has `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`

#### Scenario: xj/xslt param namespace resolves via prefix match

- **GIVEN** the xj and xslt metadata descriptors declare a `param` option with `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`
- **WHEN** the lint resolver encounters a URI key `param.foo=bar` on an `xj:` or `xslt:` endpoint
- **THEN** the key resolves to the `param` option via the Phase-2 longest-prefix match, and no `UnknownOption` diagnostic is emitted
- **AND** a bare `param.` (empty suffix) does NOT resolve and emits `UnknownOption`, matching the runtime parsers' `!param_key.is_empty()` guard

### Requirement: Open namespace URI options

The `UriOption` struct SHALL carry an optional
`pattern: Option<UriOptionMatch>` field. When `pattern` is `Some`, the option
matches URI query keys by prefix (per the `UriOptionMatch` variant) instead of
by exact name; the `name` field becomes the human label and documentation
anchor and SHALL NOT participate in matching. The `UriOptionMatch` enum SHALL
be `#[non_exhaustive]` and SHALL initially contain a single variant
`Prefix { separator: String }`.

The `camel-lint` shared resolver (`resolve_option`) SHALL determine a match in
this order, applied to a single query key:

1. **Exact-name match**, considering only options whose `pattern` field is
   `None`: the first option whose `name` equals the key wins.
2. **Alias match**, considering only options whose `pattern` field is `None`:
   the first option whose `aliases` contains the key wins.
3. **Pattern match**, considering only options whose `pattern` field is
   `Some(_)` and in order of **descending separator length**: the first option
   whose `Prefix.separator` the key starts with, AND whose remaining suffix is
   non-empty, wins. A bare `param.` key (empty suffix) does NOT match a
   `Prefix { separator: "param." }` option.

Options whose `pattern` is `Some` SHALL NOT participate in steps 1 and 2 —
their `name` and `aliases` fields are documentation-only. Options whose
`pattern` is `None` SHALL NOT participate in step 3.

**Implementation equivalence note:** steps 1 and 2 MAY be collapsed into a
single `find()` pass that tests `pattern.is_none() && (name == key ||
aliases.contains(key))`. This collapse is observationally identical to the
two-step order UNLESS a metadata alias shadows another option's exact `name`
— an authoring error the spec does not need to second-guess.

The new field SHALL serialize with `#[serde(default, skip_serializing_if =
"Option::is_none")]`, so existing JSON output remains byte-identical for
options without a pattern. The `UriOptionMatch` enum SHALL use Rust's default
externally-tagged serde representation with `#[serde(rename_all = "snake_case")]`
on the enum and on the `Prefix` variant's inner struct.

#### Scenario: patterned option name does not collide with a discrete name

- **GIVEN** a scheme has a discrete option with `name = "param"` (pattern is `None`) and a pattern option with `separator = "param."` (whose derived `name` is also `"param"`)
- **WHEN** the lint resolver evaluates a `param` query key (no suffix)
- **THEN** the key resolves to the discrete option at step 1; the pattern option is not consulted (its `name` is documentation-only and does not participate in step 1)

#### Scenario: pattern prefix matches any non-empty suffix

- **GIVEN** a `UriOption` with `name = "param"` and `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`
- **WHEN** the lint resolver evaluates a URI query key `param.foo`
- **THEN** the key resolves to that option (match succeeds with non-empty suffix `"foo"`)

#### Scenario: empty suffix does not match

- **GIVEN** a `UriOption` with `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`
- **WHEN** the lint resolver evaluates a bare `param.` key
- **THEN** the key does NOT resolve to the option, and the lint emits an `UnknownOption` diagnostic (matches the runtime parsers in `camel-xj` and `camel-xslt`, which skip empty suffixes via `!param_key.is_empty()`)

#### Scenario: discrete-name collision favors the discrete option

- **GIVEN** a scheme has two options: a discrete option whose `name = "param.foo"` and a pattern option with `separator = "param."`
- **WHEN** the lint resolver evaluates a `param.foo` query key
- **THEN** the key resolves to the discrete option (exact-name match is checked at step 1, before any pattern match at step 3)

#### Scenario: longest pattern separator wins among overlapping patterns

- **GIVEN** a scheme has two pattern options: one with `separator = "param."` and another with `separator = "param.foo."`
- **WHEN** the lint resolver evaluates a `param.foo.bar` query key
- **THEN** the key resolves to the option whose separator is `"param.foo."` (the longer separator wins)

#### Scenario: shorter pattern applies when only it matches

- **GIVEN** a scheme has two pattern options: one with `separator = "param."` and another with `separator = "param.foo."`
- **WHEN** the lint resolver evaluates a `param.baz` query key
- **THEN** the key resolves to the option whose separator is `"param."` (the longer separator does not match because the key does not start with `param.foo.`)

#### Scenario: serialization omits absent pattern

- **GIVEN** a `UriOption` constructed without a pattern
- **WHEN** the option is serialized to JSON
- **THEN** the serialized bytes do not contain a `pattern` field and match the pre-change format byte-for-byte

#### Scenario: serialization emits externally-tagged snake_case shape for Some(Prefix)

- **GIVEN** a `UriOption` with `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`
- **WHEN** the option is serialized to JSON
- **THEN** the `pattern` field equals the object `{"prefix":{"separator":"param."}}` (externally-tagged enum, snake_case variant and field names)

#### Scenario: one pattern option covers multiple distinct suffixes

- **GIVEN** a `UriOption` with `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`
- **WHEN** the lint resolver evaluates three query keys `param.a`, `param.b`, and `param.longName`
- **THEN** all three keys resolve to the same option (one namespace option covers any non-empty suffix)

### Requirement: `descriptor` attribute marks metadata-only descriptors

The `#[derive(UriConfig)]` macro SHALL accept a new bare-ident `descriptor`
flag on the struct-level `#[uri_config(..)]` attribute. When `descriptor` is
present, the macro SHALL suppress shape-based `required` inference for every
`#[uri_param]` field on that struct: a field is `required = true` ONLY when
its `#[uri_param]` attribute carries the explicit `required` key.

When `descriptor` is absent (the default), the macro SHALL retain the existing
shape-based inference unchanged: `non-Option + no default => required = true`.

The existing precedence rules are unchanged and apply regardless of
`descriptor`: `pattern` always yields `required = false` (open namespaces
cannot require a single key); explicit `required` always yields
`required = true`; `Option<T>` fields are `required = false` unless explicit
`required` is present (per the existing "Required-flag coherence with Option
types" requirement).

Rationale: in a metadata-only descriptor struct the fields are
documentation-only placeholders (underscore-prefixed by convention); the real
parser is hand-written elsewhere in the component crate. The Rust field type
is decorative and cannot carry required-intent. The explicit `descriptor`
attribute makes the author's intent visible at the struct level and survives
refactoring, unlike naming or underscore-prefix conventions.

`descriptor` is an opt-in inference mode: it instructs the macro to require
explicit `#[uri_param(.., required)]` for every required field on that struct.
It does NOT categorically distinguish "metadata-only container" from "runtime
config" — that is the author's declaration, not a property the macro infers.
A struct that does not set `descriptor` keeps shape-based inference even if
its fields happen to be underscore-prefixed or it is named `*Descriptor`.
Conversely, a runtime config struct that mistakenly sets `descriptor` would
lose required diagnostics for its bare non-Option fields — the macro does not
prevent this; the per-component parity tests (extended per the
"Per-field parity assertions" sub-section below) catch it.

Runtime config structs (including those using `skip_impl` because they have
hand-written parsers — `TimerConfig`, `CronConfig`, `SqlUriConfig`,
`OpenSearchUriConfig`, `ContainerUriConfig`, `HttpStaticUriConfig`) MUST NOT
set `descriptor`. Their field types carry required-intent because the
macro-generated or hand-written parser uses them directly.

#### Scenario: descriptor struct with bare non-Option field is not required

- **GIVEN** a struct with `#[uri_config(skip_impl, descriptor, metadata(..))]` and a field `#[uri_param(name = "foo")] pub _foo: String` (non-Option, no `default`, no explicit `required`)
- **WHEN** the macro generates the `UriOption` for this field
- **THEN** the option `required` flag is `false` (shape inference is suppressed under `descriptor`)

#### Scenario: descriptor struct with explicit required stays required

- **GIVEN** a struct with `#[uri_config(skip_impl, descriptor, metadata(..))]` and a field `#[uri_param(name = "profile", required)] pub _profile: String`
- **WHEN** the macro generates the `UriOption` for this field
- **THEN** the option `required` flag is `true` (explicit annotation wins regardless of `descriptor`)

#### Scenario: runtime config without descriptor retains shape inference

- **GIVEN** a struct with `#[uri_config(skip_impl, metadata(..))]` (no `descriptor`) and a field `#[uri_param(name = "wsdl")] pub wsdl: String` (non-Option, no `default`)
- **WHEN** the macro generates the `UriOption` for this field
- **THEN** the option `required` flag is `true` (existing behavior preserved — runtime config shape inference is unchanged)

#### Scenario: descriptor struct with default is not required

- **GIVEN** a struct with `#[uri_config(skip_impl, descriptor, metadata(..))]` and a field `#[uri_param(name = "period", default = "1000")] pub _period: u64`
- **WHEN** the macro generates the `UriOption` for this field
- **THEN** the option `required` flag is `false` (the presence of a `default` already implies not-required; unchanged)

#### Scenario: descriptor struct with Option field stays not required

- **GIVEN** a struct with `#[uri_config(skip_impl, descriptor, metadata(..))]` and a field `#[uri_param(name = "password")] pub _password: Option<String>`
- **WHEN** the macro generates the `UriOption` for this field
- **THEN** the option `required` flag is `false` (consistent with the existing "Required-flag coherence with Option types" requirement; `descriptor` does not alter Option-field behavior)

#### Scenario: descriptor struct with pattern field is not required

- **GIVEN** a struct with `#[uri_config(skip_impl, descriptor, metadata(..))]` and a field `#[uri_param(pattern = "param.")] pub _params: Vec<(String, String)>`
- **WHEN** the macro generates the `UriOption` for this field
- **THEN** the option `required` flag is `false` (open namespaces cannot require; unchanged)

### Requirement: jms metadata descriptor carries runtime defaults

The `JmsMetadataDescriptor` struct in `crates/components/camel-jms/src/metadata.rs`
SHALL set the `descriptor` flag on its `#[uri_config(..)]` attribute and SHALL
declare `default` values for the three runtime-defaulted fields, matching the
runtime parser at `crates/components/camel-jms/src/config.rs`. The default
values SHALL be the exact strings the parser's `FromStr` implementation
ACCEPTS (not Apache Camel Java constant names), so a route author can copy the
metadata default into a URI and the parser will accept it:

- `acknowledgementMode` → `default = "Auto"` (parser accepts `"Auto"` / `"auto"`; rejects `"AUTO_ACKNOWLEDGE"` — see `AcknowledgementMode::from_str` at config.rs:90)
- `transactionMode` → `default = "None"` (parser accepts `"None"` / `"none"`; rejects `"false"` — see `JmsTransactionMode::from_str` at config.rs:133)
- `exchangePattern` → `default = "InOnly"` (parser accepts `"InOnly"` / `"inOnly"` / `"in_only"` — see `ExchangePattern::from_str` at config.rs:173)

The `jms_metadata_uri_options_parity` test SHALL be extended to assert, for
every `uri_option` entry, the `required` flag (and where applicable, the
`default_value`). The three defaulted fields SHALL be asserted as
`required = false` with the matching `default_value`.

#### Scenario: jms metadata carries acknowledgementMode default

- **GIVEN** the `JmsMetadataDescriptor` with `#[uri_param(name = "acknowledgementMode", default = "Auto")]`
- **WHEN** `JmsMetadataDescriptor::metadata()` is queried
- **THEN** the `acknowledgementMode` entry's `default_value` is `Some("Auto")` (the exact string the runtime `AcknowledgementMode::from_str` accepts) and its `required` flag is `false`

#### Scenario: jms metadata carries transactionMode default

- **GIVEN** the `JmsMetadataDescriptor` with `#[uri_param(name = "transactionMode", default = "None")]`
- **WHEN** `JmsMetadataDescriptor::metadata()` is queried
- **THEN** the `transactionMode` entry's `default_value` is `Some("None")` (the exact string the runtime `JmsTransactionMode::from_str` accepts) and its `required` flag is `false`

#### Scenario: jms metadata carries exchangePattern default

- **GIVEN** the `JmsMetadataDescriptor` with `#[uri_param(name = "exchangePattern", default = "InOnly")]`
- **WHEN** `JmsMetadataDescriptor::metadata()` is queried
- **THEN** the `exchangePattern` entry's `default_value` is `Some("InOnly")` and its `required` flag is `false`

#### Scenario: jms parity test asserts required flag per field

- **GIVEN** the `jms_metadata_uri_options_parity` test runs against the migrated `JmsMetadataDescriptor`
- **WHEN** the test inspects each `uri_option`
- **THEN** every entry has its `required` flag asserted (`assert!(opt.required)` or `assert!(!opt.required)`) — no field is left without a `required`-flag assertion

### Requirement: cxf metadata descriptor reflects runtime optionality

The `CxfMetadataDescriptor` struct in `crates/components/camel-cxf/src/metadata.rs`
SHALL set the `descriptor` flag on its `#[uri_config(..)]` attribute and SHALL
reflect runtime optionality for its three currently-bare fields:

- `profile` SHALL carry explicit `required` (the runtime at
  `crates/components/camel-cxf/src/component.rs:67-68` returns a
  `CamelError::ProcessorError` when `profile` is absent).
- `attachment_content_type` SHALL be `Option<String>` (the runtime config
  field defaults to `None` and is consumed only when `mtom_enabled` is true).
- `operation` SHALL remain bare non-Option `String` and become
  `required = false` under the new `descriptor` rule. The runtime parser at
  `crates/components/camel-cxf/src/config.rs:228,292,306` stores `operation`
  as `Option<String>` defaulting to `None`; at dispatch time the
  `CamelCxfOperation` header MAY override the URI-supplied value, so the
  metadata `required = false` reflects parser behavior. (Note: there is no
  WSDL-derived default — `None` is a genuine absence that the runtime
  resolves at dispatch via the header.)

The `cxf_metadata_uri_options_parity` test SHALL be extended to assert the
`required` flag for every field. Specifically: `wsdl`, `service`, `port`,
`profile` → `assert!(opt.required)`; `operation`, `timeout_ms`,
`mtom_enabled`, `attachment_content_type` → `assert!(!opt.required)`.

#### Scenario: cxf metadata declares profile as explicitly required

- **GIVEN** the `CxfMetadataDescriptor` with `#[uri_param(name = "profile", required)] pub _profile: String`
- **WHEN** `CxfMetadataDescriptor::metadata()` is queried
- **THEN** the `profile` entry's `required` flag is `true`

#### Scenario: cxf metadata declares attachment_content_type as optional

- **GIVEN** the `CxfMetadataDescriptor` with `#[uri_param(name = "attachment_content_type")] pub _attachment_content_type: Option<String>`
- **WHEN** `CxfMetadataDescriptor::metadata()` is queried
- **THEN** the `attachment_content_type` entry's `required` flag is `false`

#### Scenario: cxf operation becomes not-required under descriptor

- **GIVEN** the `CxfMetadataDescriptor` with `descriptor` flag set and `#[uri_param(name = "operation")] pub _operation: String` (bare, no explicit `required`)
- **WHEN** `CxfMetadataDescriptor::metadata()` is queried
- **THEN** the `operation` entry's `required` flag is `false` (shape inference suppressed under `descriptor`)

#### Scenario: cxf parity test asserts required flag per field

- **GIVEN** the `cxf_metadata_uri_options_parity` test runs against the migrated `CxfMetadataDescriptor`
- **WHEN** the test inspects each `uri_option`
- **THEN** every entry has its `required` flag asserted — `wsdl`, `service`, `port`, `profile` asserted `required`; `operation`, `timeout_ms`, `mtom_enabled`, `attachment_content_type` asserted not-required

### Requirement: controlbus authorizedRoutes declared in example routes

Example route files under `examples/` that use
`controlbus:route?routeId=..&action=..` URIs SHALL include the
`authorizedRoutes` query parameter listing the routeIds the URI targets,
per the controlbus component's required `authorizedRoutes` option
(ADR-0032 fail-closed allowlist; ADR-0034 denies self-targeting).

This is an example-route correction, not a metadata change — the controlbus
metadata already correctly declares `authorizedRoutes` as required.

#### Scenario: jms.yaml controlbus URIs declare authorizedRoutes

- **GIVEN** `examples/camel-cli-run/routes/jms.yaml` contains 5 `controlbus:route` URIs targeting the routeIds `jms-producer`, `jms-consumer`, and `artemis-ready-watcher`
- **WHEN** the lint_corpus gate runs over the file
- **THEN** no `R-URI-known:missing-required-option` diagnostic is emitted for any controlbus URI in this file (each carries `&authorizedRoutes=<target_routeId>`)

#### Scenario: master.yaml controlbus URI declares authorizedRoutes

- **GIVEN** `examples/master-leader-yaml/routes/master.yaml` contains 1 `controlbus:route` URI targeting `master-route_1`
- **WHEN** the lint_corpus gate runs over the file
- **THEN** no `R-URI-known:missing-required-option` diagnostic is emitted for the controlbus URI (it carries `&authorizedRoutes=master-route_1`)

### Requirement: cxf profile declared in soap-producer example route

The `examples/cxf-example/routes/soap-producer.yaml` route SHALL include the
`profile` parameter in its `cxf://` URI, matching the cxf component's required
`profile` option (the runtime at `crates/components/camel-cxf/src/component.rs`
errors if `profile` is absent). If the example application's `Camel.toml` (or
equivalent configuration source) does not define a profile entry, a minimal
one SHALL be added so the example remains runnable.

#### Scenario: soap-producer.yaml cxf URI declares profile

- **GIVEN** `examples/cxf-example/routes/soap-producer.yaml` contains a `cxf://` URI
- **WHEN** the lint_corpus gate runs over the file
- **THEN** no `R-URI-known:missing-required-option` diagnostic is emitted for `profile` (the URI carries `&profile=<name>` and the example's configuration defines that profile)

### Requirement: Follow-up bd issues for unmigrated descriptors

The change SHALL file 5 follow-up bd issues (one each) for the descriptors
that contain bare-non-option fields but are NOT migrated in this change. Each
follow-up SHALL list the descriptor's bare fields and the migration step (add
`descriptor` flag + per-field runtime audit + extended parity test).

The 5 unmigrated descriptors (with bare-field counts) are:

- `GrpcMetadataDescriptor` (`crates/components/camel-component-grpc/src/metadata.rs`) — 14 bare fields
- `KafkaMetadataDescriptor` (`crates/components/camel-kafka/src/metadata.rs`) — 12 bare fields
- `MqttMetadataDescriptor` (`crates/components/camel-mqtt/src/metadata.rs`) — 2 bare fields
- `RedisMetadataDescriptor` (`crates/components/camel-redis/src/metadata.rs`) — 2 bare fields
- `ValidatorMetadataDescriptor` (`crates/components/camel-validator/src/metadata.rs`) — 2 bare fields

#### Scenario: five follow-up bd issues filed

- **GIVEN** the change closes bd rc-1pfm
- **WHEN** the bd issue tracker is inspected
- **THEN** 5 new open issues exist, one per unmigrated descriptor, each `discovered-from: rc-1pfm`, each listing the descriptor's bare fields and the migration step

