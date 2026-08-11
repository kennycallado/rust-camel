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

