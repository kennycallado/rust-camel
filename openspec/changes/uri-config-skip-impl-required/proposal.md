# Proposal: uri-config-skip-impl-required

## Why

The `lint_corpus` zero-false-positives gate fails on `main`. Three example
route files emit `R-URI-known:missing-required-option(error)` diagnostics
absent from the baseline (bd rc-1pfm). Root cause: the `#[derive(UriConfig)]`
macro infers `required = true` from field shape (`non-Option + no default`) at
`crates/camel-endpoint-macros/src/uri_config.rs:858-862`. This inference is
meaningful for runtime config structs (where the field IS the parsed value)
but is a footgun for **metadata-descriptor** structs (where underscore-prefixed
fields are documentation-only placeholders and the runtime parser is
hand-written elsewhere). The author's `String` vs `Option<String>` choice in a
descriptor is decorative, not intent-carrying.

e_opus consultation (recorded on rc-1pfm, KB source `rc-1pfm-eopus-ruling`)
ruled the macro should refuse shape-based `required` inference for descriptor
structs. e_gpt spec-bless corrected the discriminator: `skip_impl` alone is
too broad (it also marks runtime configs like `TimerConfig`, `CronConfig`,
`SqlUriConfig`, `OpenSearchUriConfig`, `ContainerUriConfig`). The correct
discriminator is an **explicit opt-in attribute**.

## What Changes

**In scope (this change):**

1. **New macro attribute `descriptor`** (`crates/camel-endpoint-macros`): a
   bare flag on `#[uri_config(..)]`. When set, shape-based `required`
   inference is suppressed — a field is `required = true` ONLY with explicit
   `#[uri_param(..., required)]`. Explicit `required` and `pattern`/`default`
   precedence are unchanged. Runtime config structs (no `descriptor` flag)
   keep existing shape inference byte-identical.

2. **Migrate `JmsMetadataDescriptor`** (`crates/components/camel-jms`): add
   `descriptor` flag; add `default = "Auto"` / `"None"` / `"InOnly"` to the 3
   bare fields. Runtime at `config.rs:281/284/289` initializes these via
   `*::default()` (`AcknowledgementMode::Auto`, `JmsTransactionMode::None`,
   `ExchangePattern::InOnly`); metadata was over-strict. The default strings
   are the exact tokens the runtime `FromStr` parsers accept.

3. **Migrate `CxfMetadataDescriptor`** (`crates/components/camel-cxf`): add
   `descriptor` flag; add explicit `required` to `profile` (runtime at
   `component.rs:67-68` errors if absent); change `attachment_content_type`
   to `Option<String>` (MTOM-only, runtime field defaults to `None`). The
   third bare field `operation` becomes not-required under the new rule,
   matching the runtime parser (parser stores `Option<String>` defaulting to
   `None`; `CamelCxfOperation` header may override at dispatch time).

4. **Example route fixes** (real defects per ADR-0032/0034 + runtime):
   - `examples/camel-cli-run/routes/jms.yaml`: add `authorizedRoutes` to 5
     `controlbus:route` URIs.
   - `examples/master-leader-yaml/routes/master.yaml`: add `authorizedRoutes`
     to 1 `controlbus:route` URI.
   - `examples/cxf-example/routes/soap-producer.yaml`: add `profile` to the
     `cxf://` URI (+ a `Camel.toml` profile entry if the example has none).

5. **Per-field parity assertions**: the existing `jms_metadata_uri_options_parity`
   and `cxf_metadata_uri_options_parity` tests SHALL be extended to assert the
   `required` flag for EVERY field (not just name matching), so the disposition
   is executable. Specifically: every field retained as required (cxf `profile`)
   gets a focused `assert!(opt.required)` assertion; every field relaxed (jms 3,
   cxf `operation` + `attachment_content_type`) gets `assert!(!opt.required)`.

6. **Docs**: update `crates/camel-endpoint/CONTEXT.md` `## UriConfig derive
   contract` with the new `descriptor` attribute and its semantics. Cross-link
   rc-1pfm + the e_opus ruling.

**Out of scope (follow-up bd issues, one per descriptor):**

The remaining 5 descriptors with bare-non-option fields (`GrpcMetadataDescriptor`
14, `KafkaMetadataDescriptor` 12, `MqttMetadataDescriptor` 2,
`RedisMetadataDescriptor` 2, `ValidatorMetadataDescriptor` 2 — total 32 bare
fields) are NOT migrated in this change. They keep status-quo shape inference
(no known false positives for them today). Each gets a follow-up bd issue
tracking: add `descriptor` flag + per-field runtime audit. The macro rule
works for any opt-in descriptor regardless of when they migrate.

## Acceptance criteria

- `cargo test -p camel-cli --test lint_corpus` passes on `main` with zero
  baseline changes (the 3 false positives disappear via metadata fixes; no
  new MISSING-REGRESSION failures — `http-static.dir` and other baselined
  entries are untouched because `http-static` does not opt into `descriptor`).
- Macro unit tests in `camel-endpoint-macros` cover: (a) `descriptor` struct
  with bare non-Option field → `required = false`; (b) `descriptor` struct
  with explicit `required` → `required = true`; (c) runtime config (no
  `descriptor`) with non-Option field → `required = true` (unchanged);
  (d) `descriptor` struct with `Option<T>` field → `required = false`;
  (e) `descriptor` struct with `default` field → `required = false`;
  (f) `descriptor` struct with `pattern` field → `required = false`.
- `camel-jms` parity test asserts the 3 new default values AND `required = false`.
- `camel-cxf` parity test asserts `profile.required == true`,
  `attachment_content_type.required == false`, `operation.required == false`.
- The 3 example route files pass `camel lint` with zero
  `R-URI-known:missing-required-option` diagnostics.
- `crates/camel-endpoint/CONTEXT.md` documents the `descriptor` attribute.
- 5 follow-up bd issues filed, one per unmigrated descriptor, listing its bare
  fields and the migration step.
- All AGENTS.md quality gates green.

## Risk budget

- **Accepted risk**: 5 descriptors remain on shape-inference (status quo). The
  `descriptor` attribute is opt-in, so existing code is byte-identical unless
  touched. New descriptors written after this change SHOULD use `descriptor`,
  but the macro does not enforce it (a future tightening may add a deprecation
  lint; out of scope here).
- **Out of bounds**: changing runtime parsing behavior (metadata-only change).
  Changing the lint_corpus baseline contract (still set-equality, no
  false-positive baselining).
- **Reversibility**: high. The `descriptor` attribute is additive; removing it
  from a struct restores shape inference. The metadata corrections are additive
  (`required`, `default`, `Option<T>`).
