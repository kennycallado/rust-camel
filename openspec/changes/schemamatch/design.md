# Design: schemamatch

## Approach

One enum variant, one parser arm, tests, docs. The schema compiler already
exists (`camel-json-schema`, ADR-0100); this change wires it into the unit-tier
testkit as an assertion-side body matcher.

**Scope ruling (restate, e_opus 2026-11-15, "Testkit assertions: unit tier
only"):** the declarative test document gets the `jsonSchema` body matcher.
The scenario/integration-tier `validate` target is `rc-v0bxl` (p3) and is NOT
built here. `xsd`/`protobuf` have no engine in the tree; a document naming
them is rejected at load with a message naming the supported set (json, yaml),
and the gap is a bd follow-up, not a stub.

**D1 — `BodyMatcher::JsonSchema` (camel-component-mock).** Struct variant
`JsonSchema { schema: Arc<CompiledSchema>, path: String }`. `CompiledSchema`
is not `Clone`, so the `Arc` keeps the derived `Clone`; the derived `Debug`
stays honest (`CompiledSchema` derives `Debug`). `path` is the schema path as
passed at load (document directory joined when the declared path is relative)
and is rendered by `Display` (`jsonSchema <path>`) and in failure messages.
Matching reuses the existing private `json_value` helper — no second body→JSON
path; a non-JSON body is a non-match. A public constructor
`BodyMatcher::json_schema_file(path, syntax) -> Result<Self, SchemaError>`
compiles via `camel_json_schema::compile(&SchemaSource::File(..), syntax,
FormatMode::DraftDefault)` — the single compile entry; no third compiler copy,
no unconfined path. `FormatMode::DraftDefault` is this matcher's explicit
policy choice (pass-through to `jsonschema::validator_for` semantics: the
root document's `$schema` selects the draft, formats are asserted exactly
when the draft says so); the validator component itself uses `Enforce`/
`Annotate` from its configuration — the testkit matcher takes no format knob.

**D2 — document grammar (camel-cli).** `jsonSchema` joins the strict
body-matcher key set (`bodies` entries and `expectReply.body` dual grammar).
Payload is an object with required non-empty `path` (string) and optional
`type` (`json` or `yaml`; omitted means extension-based `Auto` —
`.yaml`/`.yml` are YAML, every other extension is JSON). The `path` SHALL be
document-relative; an absolute path is rejected at parse (exit 2) naming the
field, the key, and the path — ADR-0100's absolute escape hatch covers `$ref`
targets inside the schema, not the matcher's root path. Unknown fields inside
the object, a missing/empty `path`, a null or non-string `type`, or an
unknown `type` are document errors; every rejection names the field and the
key, and unsupported schema languages (`xsd`, `protobuf`) additionally name
the rejected value and the supported set (json, yaml).
Compilation happens at document load (never per assertion) inside
`body_matcher_from_map`, mapping `SchemaError` to `TestDocError::InvalidMatcher`
with the schema path in the message.

**D3 — document-directory threading.** `parse_test_document(text)` cannot see
the document path and serde field deserializers cannot take context, so:

- New primary `parse_test_document_at(text, doc_dir: &Path)`; `parse_document`
  (the runner entry, which has the path) calls it with the document's parent
  directory. The existing `parse_test_document(text)` stays as the inline-text
  convenience with base `.` (documented CWD semantics for text-only callers;
  the runner always passes the real directory).
- The directory reaches `body_matcher_from_map` through a scoped thread-local
  context set around the single `serde_yaml::from_str` call inside
  `parse_test_document_at` and cleared on every exit path. This is the
  standard channel for context-free serde callbacks; path semantics stay
  explicit (the entry points take `doc_dir` as a parameter — nothing reads the
  ambient CWD implicitly). The alternatives were rejected on footprint: a
  full mirror struct + post-parse compile pass duplicates the entire document
  model (~150 lines, every future field twice); threading `&Path` through the
  deserializer chain is impossible without it.

**D4 — failure surfacing.** New `BodyMatcher::violation_note(&self, actual:
&Body) -> Option<String>` returns the first violation formatted
`{message} at {instance_path}` for `JsonSchema` (the shape
`camel-processor::json_schema_validate` established) and `None` for every
other matcher. `mismatch_note` returns `Some("body is not JSON")` for non-JSON
bodies. Three surfaces render the verdict:

- `assert.rs` ordered path: `received_body_with_note` widens to `Option<&str>`
  and prefers the shape note, else the violation note.
- `runner.rs` reply evaluator: the existing `reply body mismatch` line gains
  the same note chain — shape note for a non-JSON reply, else the first
  violation — so both promises of the delta's non-match scenario hold on the
  reply path too.
- `assert.rs` anyOrder path: matching is unchanged, but the `BodyNotFound`
  diagnostic gains an optional `diagnostic: Option<String>` clause populated
  only when the failing expectation is a `JsonSchema` matcher, rendering a
  deterministic candidate — the first **unmatched** body in receive order (the
  list the matching loop has consumed matches from), with its shape note or
  first violation — so a `jsonSchema` failure under `anyOrder` still names the
  schema path and the violation without diagnosing a body an earlier
  expectation already consumed (`[valid, invalid]` with two identical schema
  expectations diagnoses `invalid`). Every unmatched body fails the
  expectation when the scan returns no match, so the candidate's note chain is
  always informative. `Exact` expectations and every other matcher pass `None`
  (status quo). `MockAssertionError` is `#[non_exhaustive]` and matched
  nowhere outside the crate.

**D5 — lanes.** Add to `AGENTS.md ## COMPONENT LANES`, byte-identical to the
mission order: `camel-cli | local | cargo test -p camel-cli
--no-default-features --features flavor-regular,exec --no-fail-fast` and
`camel-component-mock | local | cargo test -p camel-component-mock`.
Declaring the `camel-cli` lane is the deliberate act the sweep's fail-safe
asks for — this order-of-operations note is recorded here so the plan's
lane task is understood as intentional, not accidental scope.

## Affected crates

- `camel-component-mock`: new variant, constructor, `violation_note`, unit
  tests; new dependency on the leaf crate `camel-json-schema`.
- `camel-cli`: grammar arm + load-time compilation + doc-dir threading in
  `commands/test/document.rs` / `document_parse.rs`; violation surfacing in
  `assert.rs` (via camel-mock) and `runner.rs`; document + driver tests.
- `camel-json-schema`: unchanged (consumed only).

## Architecture boundaries

Assertion-side only: `BodyMatcher` never changes producer behavior. ADR-0100
confinement is inherited verbatim by compilation through
`camel_json_schema::compile`; the delta references the `json-schema-validation`
spec rather than restating it. No new third-party dependency; `tempfile` (dev)
already exists for camel-cli. Docs: the testkit page gains a `jsonSchema`
section; CONTEXT-MAP's ADR-0100 consumer list gains the testkit entry.

## Deferrals

- Scenario/integration-tier `validate` target → bd `rc-v0bxl` (ruling).
- `xsd`/`protobuf` schema engines → bd follow-up (no engine in tree).

Bd: rc-0lvd3
