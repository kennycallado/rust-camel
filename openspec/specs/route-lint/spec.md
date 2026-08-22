# route-lint Specification

## Purpose
TBD - created by archiving change add-camel-lint. Update Purpose after archive.
## Requirements
### Requirement: Lint engine is runtime-free and produces span-exact diagnostics

The `camel-lint` crate SHALL expose a `LintEngine` that parses YAML/JSON source itself via
the `noyalib` CST (it SHALL NOT depend on `camel-dsl`, `camel-core`, or `camel-cli`),
constructs a span-carrying `LintRoute` view, and runs `Rule` implementations over it. Each
emitted `Diagnostic` SHALL carry a stable `DiagnosticCode`, a `Severity`, a byte-exact
`Span` (start/end offsets into the source text), a human message, and an optional `Fix`. The
engine SHALL accept the component catalog as `Arc<dyn ComponentMetadataCatalog>` and SHALL
expose no catalog constructor and no dependency on `Registry`. The engine SHALL tolerate
partial/malformed input: a document that fails syntax parsing SHALL still be reported without
panicking, and later tiers SHALL be skipped for the unparseable document. The workspace
hexagonal-architecture test SHALL be extended to assert that `camel-lint` does not depend on
`camel-core` or `camel-dsl`.

#### Scenario: Valid document yields no diagnostics

- **GIVEN** a syntactically valid, schema-valid route file whose URIs and options are all known to the catalog
- **WHEN** the engine runs all five rules over the document
- **THEN** the engine returns an empty diagnostic list

#### Scenario: Diagnostic span is byte-exact, not a line range

- **GIVEN** a route file with a step `timer:foo?bogus=1` where the unknown option key `bogus` starts at byte offset 42, and a catalog that knows `timer` (so the option is validated, not the scheme)
- **WHEN** R-URI-known runs over the document
- **THEN** the emitted `RUriKnown(UnknownOption)` diagnostic's `Span` start offset equals 42 and its end offset equals the byte after the last byte of `bogus` (not the start of the line, not the whole URI, not the whole file)

#### Scenario: Partial input does not crash the engine

- **GIVEN** a route file with a YAML syntax error that prevents construction of the route view
- **WHEN** the engine runs over the document
- **THEN** R-SYN emits a syntax diagnostic and the engine returns without panicking; R-SCHEMA and the semantic rules are skipped for the unparseable document

#### Scenario: Engine does not depend on camel-core or camel-dsl

- **GIVEN** the workspace hexagonal-architecture test is run
- **WHEN** the test checks `camel-lint`'s dependency edges
- **THEN** neither `camel-core` nor `camel-dsl` appears as a dependency of `camel-lint`

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

### Requirement: Schema asset is embedded and kept byte-equal

`camel-lint` SHALL embed the route schema as a checked-in copy at
`camel-lint/schema/route-schema.json`, included via `include_str!("../schema/route-schema.json")`
from `src/lib.rs`, and SHALL NOT read the
workspace-root schema at runtime. The `cargo xtask schema --check` gate SHALL be extended to
assert that `camel-lint/schema/route-schema.json` is byte-equal to the generated
`schemas/dsl/route-schema.json`; a mismatch fails the gate.

#### Scenario: Embedded schema matches the generated schema

- **GIVEN** the `schema --check` xtask gate runs
- **WHEN** it compares `camel-lint/schema/route-schema.json` against `schemas/dsl/route-schema.json`
- **THEN** the two files are byte-equal and the gate passes

#### Scenario: A drift in the embedded copy fails the gate

- **GIVEN** the embedded copy diverges from the generated schema
- **WHEN** `schema --check` runs
- **THEN** the gate fails, naming the two paths that diverged

### Requirement: Catalog coverage is authoritative-for-known and silent-for-unknown

The engine SHALL treat the catalog as authoritative for the schemes it contains and silent
for schemes it does not contain. "Present in the catalog" includes schemes whose registered
component yields only `minimal` metadata (no `uri_options`) — these are *known* schemes, and
R-URI-known SHALL emit no `unverified-scheme` note and no option diagnostics for them (it has
no `uri_options` to check against, so option rules stay silent rather than false-positive). For
a scheme genuinely absent from the catalog (a scheme with no registered component, e.g.
feature-gated-out or third-party/future schemes), R-URI-known SHALL emit a single informational
`unverified-scheme` note on the scheme token and SHALL emit no option diagnostics. There SHALL
be no "unknown scheme = error" diagnostic, because the absence of a registered component cannot
distinguish a typo from an unverified-but-valid scheme.

#### Scenario: Catalog entry absent emits an informational note, not option errors

- **GIVEN** a route using scheme `kafka` and a catalog that has no entry for `kafka`
- **WHEN** R-URI-known runs
- **THEN** it emits exactly one `unverified-scheme` diagnostic at severity info on the `kafka` token, and zero `unknown-option` diagnostics for the kafka step

#### Scenario: Registered-but-minimal scheme is known, not unverified

- **GIVEN** a route using scheme `redis` and a catalog whose `redis` entry is `minimal` (registered component, no `uri_options`)
- **WHEN** R-URI-known runs
- **THEN** it emits no `unverified-scheme` note and no option diagnostics (redis is a known scheme; there is simply nothing to validate)

#### Scenario: Catalog entry present with options validates options

- **GIVEN** a route using scheme `timer` and a catalog that has an entry for `timer` with `uri_options`
- **WHEN** R-URI-known runs
- **THEN** it emits no `unverified-scheme` note and validates the step's options against `timer`'s `uri_options`

### Requirement: Production lint catalog is non-empty and populated via lint-specific registration

The `camel lint` subcommand SHALL populate its production catalog by calling a NEW lint-specific
`pub fn register_builtin_components_for_lint(ctx: &mut CamelContext)` in `camel-cli`'s lib. This
function is NOT shared with `run` (whose registration is lifecycle-entangled with bridge/pool/
datasource/path handles that lint has no use for); it registers each builtin with empty/default
config, passes no-op runtime deps, and drops every handle. Because `Component::metadata()` has a
trait default returning `ComponentMetadata::minimal(scheme)` and `Registry::register()` harvests
it unconditionally, registering the builtins makes every registered scheme queryable — rich
metadata for components whose config opted into `#[uri_config(metadata(..))]`, and a
minimal-but-present entry for the rest. The `lint` command then obtains the catalog via
`ctx.metadata_catalog()` (a `RuntimeComponentMetadataCatalog`) and injects it. The production
catalog SHALL be non-empty for the built-in schemes (at least `timer`, `log`, `direct`). A test
SHALL assert that the production catalog reports an invalid `timer` option (proving the catalog
is populated and semantic validation is active, not inert). The drift between this lint list and
`run`'s list is an accepted, bounded cost (caught by the corpus baseline) and unified by a bd
follow-up.

#### Scenario: Production catalog reports an invalid timer option

- **GIVEN** a route file with a step `timer:tick?bogusOption=1` and the production catalog built by `camel lint`
- **WHEN** R-URI-known runs with the production catalog
- **THEN** an `unknown-option` diagnostic is emitted on `bogusOption` (proving `timer` metadata is present and consulted)

#### Scenario: Lint registration is lint-specific, not run's lifecycle-entangled list

- **GIVEN** the `camel-cli` source
- **WHEN** inspected
- **THEN** the `lint` command obtains its builtin component set from `register_builtin_components_for_lint`, a function that does NOT capture or return bridge/pool/datasource/path handles (those belong to `run` alone)

### Requirement: R-SYN reports syntax errors with byte-exact location

The engine SHALL detect YAML/JSON syntax errors via the `noyalib` parser and report each with
a byte-exact span derived from the parser's error location.

#### Scenario: Malformed YAML mapping reports the offending position

- **GIVEN** a route file containing a YAML sequence with an unclosed flow bracket `[`
- **WHEN** R-SYN runs over the document
- **THEN** a diagnostic with code `R-SYN` and severity error is emitted, and its span is a single byte at the parser-reported error location (the start of the offending construct), not line 1 and not the whole file

### Requirement: R-SCHEMA reports schema violations with per-keyword anchoring

The engine SHALL validate the document against the embedded `route-schema.json` using
`jsonschema` and map each violation to a byte-exact span using keyword-specific anchoring:
`type`/`enum`/`pattern`/`const`/`format` violations span the offending value;
`minimum`/`exclusiveMinimum` span the offending numeric value; `anyOf`/`oneOf` (where a value
fails all subschemas) span the value; `required` (a missing property) spans the parent object
node; `minItems`/`maxItems` span the array; `additionalProperties` spans the offending
additional key. The jsonschema violation message SHALL be carried in the diagnostic body.

#### Scenario: Wrong type for a field reports the field value

- **GIVEN** a route file where `steps` is a string instead of an array
- **WHEN** R-SCHEMA runs
- **THEN** a diagnostic with code `R-SCHEMA` is emitted whose span covers the offending string value, with the jsonschema `type` violation message in the body

#### Scenario: Missing required property reports the parent object

- **GIVEN** a route mapping that omits a property declared `required` in the schema
- **WHEN** R-SCHEMA runs
- **THEN** a diagnostic with code `R-SCHEMA` is emitted whose span covers the parent mapping node (because no offending leaf exists), with the jsonschema `required` violation message in the body

#### Scenario: minimum violation reports the numeric value

- **GIVEN** a numeric field whose value is below the schema's `minimum`, where the value starts at byte offset 50
- **WHEN** R-SCHEMA runs
- **THEN** a diagnostic with code `R-SCHEMA` is emitted whose span start offset is 50 (the numeric value), with the `minimum` violation message in the body

#### Scenario: anyOf failure reports the value

- **GIVEN** a field constrained by `anyOf` whose value matches none of the subschemas
- **WHEN** R-SCHEMA runs
- **THEN** a diagnostic with code `R-SCHEMA` is emitted whose span covers the value, with the `anyOf` violation message in the body

### Requirement: R-URI-known validates options against catalog metadata for known schemes

For each step URI whose scheme is present in the catalog, R-URI-known SHALL resolve each URI
option against `uri_options`: an option not matching any `name` or `alias` emits an
`unknown-option` error on the option key; a `required` option that is absent emits a
`missing-required-option` error on the step URI; an option whose value type does not match
its declared `OptionKind` (e.g. an `OptionKind::Bool` option given a non-boolean string)
emits a `kind-mismatch` error. Options matching a declared `alias` SHALL be accepted without
a diagnostic and normalized to the canonical option name for kind checking.

#### Scenario: Unknown option for a known scheme reported

- **GIVEN** a step `timer:foo?frequency=1s` and a catalog whose `timer` metadata lists option `period` but neither an option nor alias named `frequency`
- **WHEN** R-URI-known runs
- **THEN** a diagnostic with code `R-URI-known`, sub-code `unknown-option`, severity error, is emitted on the `frequency` option key

#### Scenario: Missing required option reported

- **GIVEN** a step `timer:foo` and a catalog whose `timer` metadata declares option `period` as `required = true`
- **WHEN** R-URI-known runs
- **THEN** a diagnostic with code `R-URI-known`, sub-code `missing-required-option`, severity error, is emitted on the `timer:foo` URI

#### Scenario: Accepted alias is not reported

- **GIVEN** a step using an option key that is a declared `alias` of a catalog option, with a value matching the option's kind
- **WHEN** R-URI-known runs
- **THEN** no diagnostic is emitted for that option

#### Scenario: Kind mismatch reported

- **GIVEN** a step using a catalog option declared `OptionKind::Bool` with a string value `maybe`
- **WHEN** R-URI-known runs
- **THEN** a diagnostic with code `R-URI-known`, sub-code `kind-mismatch`, severity error, is emitted on the option value

### Requirement: R-SECRET flags secret options set to literal values

For each option flagged `secret = true` in the catalog, R-SECRET SHALL examine the provided
value. A value that does not match an interpolation/reference pattern recognized by the DSL
(`${...}` environment interpolation, or `{{...}}` placeholder interpolation) SHALL be treated
as a literal and emit a `literal-secret` warning on the value. R-SECRET SHALL NOT emit an
error merely because a secret option is absent (absence is a `missing-required-option`
concern owned by R-URI-known when the option is also `required`).

#### Scenario: Secret option provided as literal string warned

- **GIVEN** a step whose `password` option is `secret = true` in the catalog and the route sets `password=hunter2` (no interpolation markers)
- **WHEN** R-SECRET runs
- **THEN** a diagnostic with code `R-SECRET`, sub-code `literal-secret`, severity warning, is emitted on the value `hunter2`

#### Scenario: Secret option provided as interpolation is not warned

- **GIVEN** a step whose `password` option is `secret = true` and the route sets `password={{ secrets.db.password }}`
- **WHEN** R-SECRET runs
- **THEN** no `R-SECRET` diagnostic is emitted for that option

### Requirement: R-DEPRECATED flags deprecated options

For each option whose catalog `UriOption.deprecated` field is set (an `Option<String>`
carrying a deprecation message), R-DEPRECATED SHALL emit a warning on the option key naming
the deprecation message. Scheme-level deprecation is out of scope (no field exists on
`ComponentMetadata` today) and is deferred to a bd follow-up that first extends the metadata.

#### Scenario: Deprecated option reported with its deprecation message

- **GIVEN** a catalog where option `oldFreq` has `deprecated = Some("use \`period\` instead")` and a route using `oldFreq`
- **WHEN** R-DEPRECATED runs
- **THEN** a diagnostic with code `R-DEPRECATED`, severity warning, is emitted on the `oldFreq` key, carrying the deprecation message in its body

### Requirement: `camel lint` CLI runs the engine and exits by severity

The `camel lint` CLI subcommand (in `camel-cli`) SHALL construct the production catalog via
`register_builtin_components_for_lint` (obtained from `ctx.metadata_catalog()` as a
`RuntimeComponentMetadataCatalog`), inject it into
`LintEngine::new(...)`, run the engine over the given file(s), render diagnostics with
`ariadne`, and exit 0 when clean, 1 when any error-severity diagnostic is present, and 2 on
engine or CLI misuse (e.g. an unreadable or missing file).

#### Scenario: Clean route exits zero

- **GIVEN** a valid route file with no diagnostics
- **WHEN** `camel lint route.yaml` runs
- **THEN** the process exits 0 and prints nothing

#### Scenario: Route with an error exits one

- **GIVEN** a route file that produces an error-severity diagnostic
- **WHEN** `camel lint route.yaml` runs
- **THEN** the process prints an ariadne-rendered diagnostic and exits 1

#### Scenario: Unreadable file exits two

- **GIVEN** a path that does not exist
- **WHEN** `camel lint missing.yaml` runs
- **THEN** the process exits 2 with a CLI-error message

### Requirement: Zero false positives over a discovered in-tree corpus

The `camel-cli` integration test `tests/lint_corpus.rs` SHALL discover every route file in the
repository by a glob rule (covering `examples/**/*.{yaml,json}` and
`crates/**/tests/fixtures/**/*.{yaml,json}`, plus any route fixtures referenced by the
schema-validation corpus), run the engine with the production catalog over each, and compare
the emitted diagnostics against a checked-in baseline file
`tests/fixtures/lint-corpus-baseline.ron` (parsed with the `ron` crate, a `camel-cli`
dev-dependency). The test SHALL fail if any emitted diagnostic is absent from the baseline (a
false positive) or any baseline diagnostic is missing (a regression). Baseline updates are
reviewed diffs. The corpus file count is discovered at test-run time (not hardcoded). The
corpus SHALL include at least one fixture exercising a secret inside a `parameters:` map, and
its baseline entry SHALL pin the emitted R-SECRET diagnostic. This is
the merge gate: a rule that cannot meet zero false positives on the corpus SHALL be gated (the
`unverified-scheme` guard) or cut before merge.

#### Scenario: Corpus run matches the checked-in baseline

- **GIVEN** the engine built with the production catalog and all five rules active, and the checked-in baseline
- **WHEN** `tests/lint_corpus.rs` runs over the discovered corpus
- **THEN** the set of emitted diagnostics equals the baseline set exactly; the test passes

#### Scenario: A new false positive fails the gate

- **GIVEN** a change to the engine that emits a diagnostic against a corpus file not present in the baseline
- **WHEN** `tests/lint_corpus.rs` runs
- **THEN** the test fails, naming the file and diagnostic code that is outside the baseline

#### Scenario: Secret in parameters map is diagnosed with an in-map span

- **GIVEN** a corpus fixture where a catalog-secret option is set inside a `parameters:` map
- **WHEN** the engine lints the fixture
- **THEN** R-SECRET is emitted with a span pointing at the value inside the `parameters:` map, and the baseline contains that entry

### Requirement: Document supports incremental range edits

The `Document` struct SHALL expose `apply_edit(&mut self, start: usize, end: usize,
replacement: &str) -> Result<(), LintError>` which replaces the byte range
`[start, end)` in the source with `replacement`, re-parses the result, and **always
commits** the new state — including when the re-parse produces a `parse_failure`.
This mirrors an editor's live state: intermediate edits routinely produce invalid
syntax (e.g. typing `from:` before the value), and the server's document MUST
reflect the editor's actual text so R-SYN can report the syntax error.

`apply_edit` differs from `apply_fix` in this commitment semantic:

- **`apply_edit`** (LSP didChange) — always commits. The new `raw`, `route_view`,
  and `parse_failure` reflect the post-edit state regardless of validity. Returns
  `Err` ONLY for structural problems that prevent applying the edit at all
  (out-of-bounds range, non-character-boundary offset).
- **`apply_fix`** (automated lint fix) — transactional. Delegates to `apply_edit`
  for the byte replacement, but rolls back to the pre-edit `Document` if the result
  has a `parse_failure` (an automated fix must never break syntax). Returns `Err`
  on rollback.

`apply_edit` SHALL be total over its inputs: out-of-bounds ranges and
non-character-boundary offsets SHALL return `Err` without mutating the document,
and no input SHALL cause a panic.

#### Scenario: apply_edit replaces a byte range and updates the route view

- **GIVEN** a `Document` parsed from `from: direct:start\n` and an edit replacing byte offsets 12–17 (`start`) with `end`
- **WHEN** `apply_edit(12, 17, "end")` is called
- **THEN** the document's `raw` field equals `from: direct:end\n`, its `route_view.from` reflects the new value, and `parse_failure` is `None`

#### Scenario: apply_edit commits a syntax-breaking edit and records the failure

- **GIVEN** a `Document` parsed from a valid route, and a replacement that produces an unclosed YAML bracket
- **WHEN** `apply_edit` is called with that replacement
- **THEN** the result is `Ok(())`, the document's `raw` reflects the edited text, its `parse_failure` is `Some(_)`, and a subsequent `LintEngine::lint` over `doc.raw` emits an R-SYN diagnostic

#### Scenario: apply_edit recovers from invalid to valid

- **GIVEN** a `Document` whose `parse_failure` is `Some(_)` (currently broken), and a replacement that fixes the syntax
- **WHEN** `apply_edit` is called with that replacement
- **THEN** the result is `Ok(())`, `parse_failure` becomes `None`, and `route_view` reflects the now-valid structure

#### Scenario: apply_edit rejects out-of-bounds range

- **GIVEN** a `Document` parsed from a 20-byte source
- **WHEN** `apply_edit(0, 25, "x")` is called (end exceeds source length)
- **THEN** the result is `Err` and the document is byte-identical to its pre-edit state

#### Scenario: apply_fix delegates to apply_edit and rolls back on parse_failure

- **GIVEN** the refactored `apply_fix` implementation
- **WHEN** `apply_fix(fix)` is called and the resulting edit produces a `parse_failure`
- **THEN** `apply_fix` returns `Err`, and the document is byte-identical to its pre-fix state (the transactional rollback distinguishes it from `apply_edit`)

### Requirement: Engine provides completion candidates at a byte offset

The `LintEngine` SHALL expose `complete_at(&self, doc: &Document, offset: usize) ->
Vec<CompletionItem>`. The engine inspects `doc.route_view` to locate the cursor
context:

1. **Scheme position** — cursor is in or immediately after a scheme token (before
   the `:` separator). Returns one `CompletionItem` per catalog scheme name.
2. **Option-key position** — cursor is in the query-string region after `?` or `&`,
   in a position that is or follows an option key. Returns one `CompletionItem` per
   declared option name and alias for the resolved scheme.
3. **Option-value position** — cursor is in an option value. Returns kind-
   appropriate defaults (bool → `true` / `false`; other kinds → empty list).
4. **No context** — cursor is outside any URI span. Returns an empty list.

The method SHALL NOT panic for any offset, including offsets beyond the source
length. For a scheme whose catalog entry is `minimal` (no `uri_options`), the
option-key position SHALL return an empty list (graceful, not an error).

#### Scenario: Scheme position offers catalog scheme names

- **GIVEN** a document with `from: tim` (cursor at byte 10, inside the scheme token `tim`) and a catalog containing `timer`, `log`, `direct`
- **WHEN** `complete_at` is called with offset 10
- **THEN** the result includes `timer` (and `log`, `direct`) as completion candidates

#### Scenario: Option-key position offers declared options for the resolved scheme

- **GIVEN** a document with `from: timer:tick?per` (cursor at byte 19, inside `per`) and a catalog whose `timer` entry declares option `period`
- **WHEN** `complete_at` is called with offset 19
- **THEN** the result includes `period` as a completion candidate

#### Scenario: Minimal-metadata scheme returns empty option-key completions

- **GIVEN** a document with `from: redis:cache?op` (cursor inside `op`) and a catalog whose `redis` entry is `minimal` (no `uri_options`)
- **WHEN** `complete_at` is called at that offset
- **THEN** the result is an empty list (no panic, no error)

#### Scenario: Cursor outside any URI returns an empty list

- **GIVEN** a document where the cursor is in a non-URI region (e.g. a YAML key like `steps:`)
- **WHEN** `complete_at` is called at that offset
- **THEN** the result is an empty list

#### Scenario: Offset beyond source length returns an empty list

- **GIVEN** a 20-byte document
- **WHEN** `complete_at` is called with offset 50
- **THEN** the result is an empty list (no panic)

### Requirement: Engine provides hover information at a byte offset

The `LintEngine` SHALL expose `hover_at(&self, doc: &Document, offset: usize) ->
Option<HoverInfo>`. The engine locates the option key at the cursor offset within
a URI query string, resolves it against the catalog, and returns a `HoverInfo`
struct carrying: the option's `description` (if present), the `deprecated` reason
(if the option is deprecated), and the `secret` flag (if the option is marked
secret). Returns `None` when the cursor is not on an option key, when the scheme
has no metadata, or when the option is unknown.

#### Scenario: Hover on a documented option returns its description

- **GIVEN** a document with `from: timer:tick?period=1s` and a catalog whose `timer` option `period` has `description = Some("Tick interval")`
- **WHEN** `hover_at` is called with an offset inside the `period` key
- **THEN** the result is `Some(HoverInfo { description: Some("Tick interval"), .. })`

#### Scenario: Hover on a deprecated option returns the deprecation reason

- **GIVEN** a document using option `oldFreq` and a catalog where `oldFreq` has `deprecated = Some("use \`period\` instead")`
- **WHEN** `hover_at` is called with an offset inside `oldFreq`
- **THEN** the result carries the deprecation reason in its `deprecated` field

#### Scenario: Hover on a secret option returns the secret flag

- **GIVEN** a document using option `password` and a catalog where `password` has `secret = true`
- **WHEN** `hover_at` is called with an offset inside `password`
- **THEN** the result has `secret = true`

#### Scenario: Hover outside any option key returns None

- **GIVEN** a document where the cursor is in the scheme part or outside any URI
- **WHEN** `hover_at` is called at that offset
- **THEN** the result is `None`

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

