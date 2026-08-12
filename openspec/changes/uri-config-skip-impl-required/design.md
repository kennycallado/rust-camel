# Design: uri-config-skip-impl-required

## Approach

The change introduces a new opt-in macro attribute (`descriptor`) that
distinguishes metadata-only descriptor structs from runtime config structs.
When `descriptor` is set, the macro refuses to infer `required` from field
shape. This closes the foot-gan identified by e_opus + e_gpt with the
explicit discriminator e_gpt's blocker #1 demands.

### Macro rule change

`crates/camel-endpoint-macros/src/uri_config.rs:858-862` currently:

```rust
let required = if attr.pattern.is_some() {
    false
} else {
    attr.required || (!is_option && attr.default.is_none())
};
```

Becomes (pseudocode — `is_descriptor` is parsed from the struct-level
`#[uri_config(..)]` attribute and threaded into the per-field required
computation):

```rust
let required = if attr.pattern.is_some() {
    false
} else if attr.required {
    true                            // explicit always wins
} else if is_descriptor {
    false                           // descriptor: refuse shape inference
} else {
    !is_option && attr.default.is_none()  // runtime config: shape inference preserved
};
```

The `descriptor` flag is a new bare-ident accepted in `#[uri_config(..)]`,
alongside the existing `skip_impl`, `metadata(..)`, `crate = ..` keys. It is
parsed in the same `parse_uri_config_attr` function that already parses
`skip_impl` (see `crates/camel-endpoint-macros/src/uri_config.rs:115-120`
for the `skip_impl` precedent — `descriptor` follows the identical pattern).
No new public API; the flag is macro-internal.

### Why an explicit attribute (not a heuristic)

e_gpt's blocker #1 ruled out `skip_impl` as the discriminator: 5 runtime
configs (`TimerConfig`, `CronConfig`, `SqlUriConfig`, `OpenSearchUriConfig`,
`ContainerUriConfig`) also use `skip_impl` because they have hand-written
`from_uri` parsers. Heuristics like "_-prefixed fields" or "struct name ends
in `MetadataDescriptor`" are conventions and fragile. An explicit opt-in
attribute is unambiguous and survives refactoring. The author declares
intent at the struct level; the macro respects it.

### Metadata corrections

**`camel-jms/src/metadata.rs`**: add `descriptor` to the struct's
`#[uri_config(..)]`; add `default = "Auto"` / `"None"` / `"InOnly"` to the 3
bare fields. Runtime at `config.rs:281/284/289` initializes from
`*::default()` (`AcknowledgementMode::Auto`, `JmsTransactionMode::None`,
`ExchangePattern::InOnly`). The default strings are the exact tokens the
runtime `FromStr` parsers accept (verified at config.rs:90/133/173), so a
route author can copy the metadata default into a URI and the parser accepts
it.

**`camel-cxf/src/metadata.rs`**: add `descriptor`; add explicit `required` to
`profile` (was implicit-by-shape; runtime at `component.rs:67-68` errors if
absent — preserve the correct diagnostic); change
`attachment_content_type: String` → `Option<String>` (runtime field is
`Option<String>` defaulting to `None`, consumed only when `mtom_enabled`).
The third bare field `operation` becomes not-required under the new rule,
matching the runtime parser: `config.rs:228,292,306` stores `operation` as
`Option<String>` defaulting to `None`; the `CamelCxfOperation` header may
override at dispatch time. There is no WSDL-derived default.

### Per-field parity assertions (e_gpt blocker #2)

The existing `*_metadata_uri_options_parity` tests only assert name sets.
This is insufficient: a field can be present in metadata with the wrong
`required` flag and the test passes. The change extends both tests to assert
`required` flag per field:

```rust
// In jms test:
let am = find("acknowledgementMode");
assert!(!am.required, "acknowledgementMode has a default");
assert_eq!(am.default_value.as_deref(), Some("Auto"));

// In cxf test:
let profile = find("profile");
assert!(profile.required, "profile must be required (runtime errors)");
let attachment = find("attachment_content_type");
assert!(!attachment.required, "attachment_content_type is MTOM-only");
let operation = find("operation");
assert!(!operation.required, "operation is Option<String> at runtime");
```

Every field in jms + cxf gets either `assert!(opt.required)` or
`assert!(!opt.required)` — exhaustive per-field disposition.

### Example route fixes

- `controlbus:route?routeId=X&action=Y` URIs in `jms.yaml` (5 sites) and
  `master.yaml` (1 site): append `&authorizedRoutes=<routeId>` per ADR-0032.
  The allowlist names the routeIds the URI targets (jms-producer,
  jms-consumer, artemis-ready-watcher, master-route_1).
- `cxf://...` URI in `soap-producer.yaml`: append `&profile=<name>`. The
  example's `Camel.toml` (if present) gets a minimal profile entry; if the
  example has no `Camel.toml`, the example's structure dictates the addition.

### Docs

Update `crates/camel-endpoint/CONTEXT.md` `## UriConfig derive contract` with
a new sub-section documenting the `descriptor` attribute:

> **`descriptor` flag**: When set on `#[uri_config(..)]`, the macro treats the
> struct as a metadata-only descriptor (not a runtime config). Shape-based
> `required` inference is suppressed: a field is `required = true` ONLY with
> explicit `#[uri_param(..., required)]`. Use this for private
> `XxxMetadataDescriptor` structs whose underscore-prefixed fields are
> documentation-only. Runtime config structs MUST NOT set `descriptor` —
> their field types carry required-intent because the macro-generated (or
> hand-written `skip_impl`) parser uses them.

Cross-link rc-1pfm + KB source `rc-1pfm-eopus-ruling`.

## Affected crates

- `camel-endpoint-macros`: parse `descriptor` flag in `parse_uri_config_attr`;
  thread into per-field `required` computation. 6 new unit tests (the (a)-(f)
  cases from acceptance criteria).
- `camel-endpoint`: CONTEXT.md updated; no source change.
- `camel-jms`: `descriptor` flag added; 3 `default` annotations; parity test
  extended with per-field `required` + `default_value` assertions.
- `camel-cxf`: `descriptor` flag added; `profile` gains explicit `required`;
  `attachment_content_type` → `Option<String>`; parity test extended.
- `camel-cli`: no source change; `lint_corpus` gate passes naturally.
- `examples/`: 3 route files corrected.

## Architecture boundaries

- **Macro crate** (`camel-endpoint-macros`): owns the new `descriptor` flag
  and the required-inference rule. The flag is macro-internal; no new trait
  or public type.
- **Component crates** (`camel-jms`, `camel-cxf`): descriptor structs are
  `pub(super)`; corrections are local. Runtime parsing code is untouched.
- **Lint engine** (`camel-lint`): zero source change. Consumes the corrected
  catalog.
- **CLI corpus gate** (`camel-cli/tests/lint_corpus.rs`): zero source change,
  zero baseline change. Passes because the engine emits fewer false-positive
  diagnostics.
- **`camel-timer`, `camel-cron`, `camel-sql`,
  `camel-opensearch`, `camel-container`, `camel-http` (`HttpStaticUriConfig`)**: zero change. They do not set
  `descriptor`; shape inference is preserved.

## Phases (optional)

Omitted — single-phase. The macro rule + 2 descriptor migrations + example
fixes + docs land in one coherent slice. No milestone grouping benefit.

## Alternatives considered

- **Status quo**: document the footgun. Rejected by e_opus — failed twice in
  one commit (jms + cxf).
- **Global invert** (e_opus option (b)): `required` ONLY by explicit
  annotation everywhere. Rejected by e_opus — silently suppresses real
  missing-required diagnostics in runtime configs (the `http-static` baseline
  entries depend on shape inference).
- **`skip_impl` as discriminator** (original e_opus option (c)): rejected by
  e_gpt — too broad; 5 runtime configs use `skip_impl`.
- **Heuristic discriminator** (`_`-prefixed fields or `MetadataDescriptor`
  naming): rejected — conventions are fragile under refactoring; explicit
  attribute survives.
- **Compile-error migration** (force every descriptor field to declare
  intent): stronger than the chosen approach but higher migration cost. The
  opt-in `descriptor` attribute + per-field parity tests achieve the same
  safety for migrated descriptors without forcing a workspace-wide audit.
- **Migrate all 7 bare-containing descriptors in this change**: rejected —
  scope creep. The 5 descriptors with no known false positives (grpc, kafka,
  mqtt, redis, validator) migrate via follow-up bd issues, one per descriptor,
  each with its own runtime audit. This change unblocks the gate with the
  minimal coherent slice.
