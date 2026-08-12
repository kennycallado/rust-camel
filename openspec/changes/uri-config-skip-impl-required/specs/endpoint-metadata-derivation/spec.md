## ADDED Requirements

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
