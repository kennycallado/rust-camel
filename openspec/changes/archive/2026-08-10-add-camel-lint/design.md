# Design: add-camel-lint

## Approach

`camel-lint` is a **runtime-free analysis engine**. It parses YAML/JSON itself through the
`noyalib` CST, builds a span-carrying `LintRoute` view, and validates that view against an
injected `Arc<dyn ComponentMetadataCatalog>`. It does not depend on `camel-dsl` (which pulls
`camel-core`/`camel-processor` transitively) or `camel-core`. Reusing the checked-in
`route-schema.json` for schema parity is done by **embedding a copy inside `camel-lint`**
(see Schema asset below), not by depending on the compiler.

Three analysis tiers, each contributing byte-exact spans:

1. **Syntax tier** — `noyalib` parses the source into a CST (`noyalib::cst::parse_document`,
   `Document::span_at`/`key_span`/`replace_span`, `error::Location` — present at noyalib 0.0.18).
   Syntax errors carry `error::Location`, mapped to byte offsets. Source of R-SYN.
2. **Schema tier** — the document is validated against the embedded `route-schema.json` via
   `jsonschema` (the `jsonschema` + `validator_for` pattern used in
   `camel-dsl/tests/schema_validation.rs`). Each violation maps to a span by keyword-specific
   anchoring (see R-SCHEMA in the spec). Source of R-SCHEMA.
3. **Semantic tier** — the engine walks the CST to build `LintRoute`, then resolves each URI
   scheme against the catalog. For known schemes, options are checked against `uri_options`,
   `aliases`, `kind`, `required`, `secret`, `deprecated`. Sources R-URI-known, R-SECRET,
   R-DEPRECATED.

The engine is **stateless**: `LintEngine::lint(&self, source)` parses a `Document { raw,
route_view, parse_failure }`, runs the rules, and returns the diagnostics without retaining
the document. Rules are `&dyn Rule` producing protocol-agnostic `Diagnostic { code, severity,
span, message, fix }`. The CLI is a thin shell: construct catalog → `LintEngine::new(catalog)`
→ run → render with ariadne → exit code. Incremental editing is a DOCUMENT-level operation:
`Document::apply_fix(&mut self, &Fix)` uses `cst::Document::replace_span` + re-parse of the
affected region, and the caller re-runs `engine.lint(&document.raw)` for refreshed
diagnostics. The engine exposes no `apply_edit` method and holds no document field.

### LintRoute extraction (all URI-bearing locations, spanned)

`LintRoute` is built by walking the CST (or noyalib's span-preserving deserialization). It
captures, with byte-exact spans, every location a rule reports on:

- the route-level `from` URI;
- each step's `to` / `uri` value;
- nested child steps (`choice` with `when`/`otherwise` branches, `multicast`, `scatter_gather.endpoints` — the containers present in `route-schema.json`; `pipeline` does not exist), recursively;
- each URI option key and value, parsed out of the URI query string or the step's option map.

Each captured item is `Spanned<T>` (value + span into `raw`). The exact set of step containers
(field names like `steps`, `choice`, `when`, `otherwise`, `multicast`, `scatter_gather`) is
re-derived from `route-schema.json`, not from `camel-dsl`'s
runtime AST.

### Catalog construction (in `camel-cli`, not in `camel-lint`)

**Ruling (Option A — lint-specific runtime registration).** `camel-lint` exposes no catalog
constructor and no dependency on `Registry`. The production catalog is built in the `camel lint`
subcommand through a **NEW `pub fn register_builtin_components_for_lint(ctx: &mut CamelContext)`** in
`camel-cli`'s lib — a *lint-specific* registration function, NOT a shared extraction from
`run`. The `lint` command then calls `ctx.metadata_catalog()` to obtain a
`RuntimeComponentMetadataCatalog` and injects it as `Arc<dyn ComponentMetadataCatalog>`.

Two facts settle this over the alternatives:

1. **`run`'s registration is not mechanically extractable.** It is lifecycle-entangled:
   validator/xslt/xj extract bridge handles for `BridgeCleanup`; jms/cxf capture pool handles
   for shutdown; sql/surrealdb thread a datasource catalog via `with_catalog(..)`; wasm needs a
   config-relative `base_dir`; exec is registered conditionally on discovered routes. `run`
   needs those handles; `lint` needs none of them. A single shared function that returns the
   registry *and* the handle bundle would force `lint` to fabricate and drop runtime deps it has
   no use for. The lint function registers each builtin with empty/default config, passes no-op
   runtime deps, and drops every pool/bridge/lifecycle handle.

2. **Registration yields full coverage — strictly more than static `Config::metadata()`
   calls (Option B).** `Component::metadata()` has a trait default returning
   `ComponentMetadata::minimal(scheme)` (`camel-component-api/src/component.rs`), and
   `Registry::register()` harvests it unconditionally. So every registered scheme is queryable:
   *rich* metadata for the ~8 components whose config opted into `#[uri_config(metadata(..))]`
   (timer, log, direct, sql, ws, file, http, mock, container, ...), and a *minimal-but-present*
   entry for the rest (redis, controlbus, seda, validator, ...). Option B could only ever cover
   the opted-in subset, so it is a strict regression in scheme coverage for no dep-graph win —
   `camel-cli` already depends on `camel-core` + every component crate, so Option A adds **no
   new dep edge** and, critically, **no dep edge onto `camel-lint`** (the catalog crosses the
   boundary as a trait object; `camel-lint` stays `camel-api`-only).

**`unverified-scheme` fallback.** It fires only for schemes the lint catalog cannot register at
all — i.e. schemes with no in-tree component registered by the lint function (feature-gated-out
components, third-party/future schemes). Components that require real runtime coupling and are
therefore intentionally skipped in the lint registration (or registered but yielding only
minimal metadata) still resolve as *known* — R-URI-known simply has no `uri_options` to check
against, so option-level rules stay silent rather than false-positive. This keeps lint honest:
an unregistered `foo:` scheme surfaces as an informational `unverified-scheme` note, never a
hard error, and a registered-but-metadata-thin scheme (redis/exec/wasm) is validated at the
scheme level only. No component is ever silently treated as invalid.

**Drift.** The lint registration list will drift from `run`'s list over time. That is an
accepted, bounded cost (the corpus zero-false-positive gate catches a *missing* scheme as a new
`unverified-scheme` note against the baseline). Unifying the two lists — or replacing both with
a reliable static enumeration — is a **bd follow-up**, not part of this change. Static
`inventory`/`linkme` enumeration remains out of scope (DCE footgun).

### Schema asset (packaged inside `camel-lint`)

`include_str!` against the workspace-root `schemas/dsl/route-schema.json` is not viable for a
packaged crate (the file is outside the crate and absent in an installed binary). Instead,
`camel-lint` keeps a checked-in copy at `camel-lint/schema/route-schema.json`, embeds it with
`include_str!("../schema/route-schema.json")` from `src/lib.rs`, and the existing
`cargo xtask schema --check` gate
is extended to assert byte-equality between the crate's copy and the generated
`schemas/dsl/route-schema.json`. This keeps the lint crate self-contained while preventing
silent drift.

## Affected crates

- **`camel-lint` (NEW):** `Document`, `LintRoute`, `Rule` trait, `Diagnostic`, five rule
  impls, `LintEngine`. Deps: `camel-api` (catalog trait + metadata data types, accessed via
  `camel_api::component_metadata::*`), `noyalib`, `jsonschema`, `ariadne`, `serde`,
  `serde_json`. The schema copy lives at `camel-lint/schema/route-schema.json`.
- **`camel-cli`:** add `lint` subcommand (mirrors `openapi.rs`); add a runtime dep on
  `camel-lint`; add the `ron` dev-dep for the corpus baseline; add a NEW lint-specific `pub fn
  register_builtin_components_for_lint(ctx)` (registers builtins with default config, drops all
  runtime/lifecycle handles — NOT a shared extraction from `run`); add integration test
  `tests/lint_corpus.rs` + baseline fixture `tests/fixtures/lint-corpus-baseline.ron`.
- **`camel-api`:** no change — consumes the existing `component_metadata` module as-is.
- **`camel-dsl`:** no change — lint does not depend on it.
- **`scripts/xtask`:** `schema --check` extended to assert `camel-lint/schema/route-schema.json`
  equals the generated schema.

## Architecture boundaries

`camel-lint` sits at the **contract-analysis layer**, strictly outside the runtime and the DSL
compiler. Its only in-tree dep is `camel-api` (types + catalog trait); it depends on neither
`camel-dsl` nor `camel-core` nor `camel-cli`. This respects the data/control-plane split: lint
is build-time analysis. The catalog flows in as a trait object, so the engine is testable with
a stub catalog and decoupled from how the production catalog is populated. The workspace
hexagonal-architecture test is extended to assert `camel-lint` does not depend on `camel-core`
or `camel-dsl`.

## Phases

Three phases, planned together and blessed once. Inter-phase `r_glm` review runs after Phase 1
and Phase 2 (each has ≥2 tasks); Phase 3 gets per-task reviews plus the final holistic review.

### Phase 1: Engine scaffolding
- **Goal:** crate, `Document`, `LintRoute` span-carrying view (all URI-bearing locations),
  `Rule` trait, `Diagnostic`, catalog injection, schema asset + byte-equality gate,
  architecture-boundary test. No rules yet.
- **Dependencies:** ADR-0041 (catalog trait + metadata); `noyalib` CST API
  (`span_at`/`key_span`/`replace_span`); the existing `route-schema.json`.
- **Externally-visible types:** `Document`, `LintRoute`, `LintNode`, `Endpoint`, `Rule`, `Diagnostic`,
  `LintEngine`, `Severity`, `DiagnosticCode`, `Span`.
- **Deliverable:** `camel-lint` compiles; engine runs an empty rule set over a fixture and
  returns no diagnostics; schema copy byte-equal; boundary test asserts no camel-core/camel-dsl
  dep.
- **Exit-criteria:** `cargo build -p camel-lint`; unit tests for `Document` parse + span
  resolution (including `from`, nested steps, options) pass; `cargo xtask schema --check` green;
  extended architecture test green.

### Phase 2: Rules (5)
- **Goal:** implement R-SYN, R-SCHEMA, R-URI-known, R-SECRET, R-DEPRECATED with a stub catalog.
- **Dependencies:** Phase 1; `ComponentMetadataCatalog` semantics (`uri_options`, `aliases`,
  `kind`, `required`, `secret`, `deprecated`).
- **Externally-visible types:** five `Rule` impls + their `DiagnosticCode` variants.
- **Deliverable:** each rule with executable tests (arrange/act/assert on fixtures) asserting
  byte-exact spans; R-SCHEMA per-keyword anchoring (covering type/enum/pattern/const/format/
  minimum/anyOf/oneOf value-anchored, required parent-anchored, items array-anchored,
  additionalProperties key-anchored).
- **Exit-criteria:** all rule unit tests green; `cargo clippy -p camel-lint -- -D warnings`
  clean; spans byte-exact (asserted); `unverified-scheme` guard tested.

### Phase 3: CLI + zero-false-positives gate
- **Goal:** `camel lint` subcommand with a populated production catalog; corpus discovery +
  checked-in baseline; add lint-specific `register_builtin_components_for_lint` (independent of
  `run.rs`, no lifecycle-handle extraction).
- **Dependencies:** Phase 2; `camel-cli` command pattern; `run.rs` registration list as the
  reference set for the lint list.
- **Externally-visible types:** `camel lint` subcommand flags;
  `register_builtin_components_for_lint`; exit codes 0/1/2.
- **Deliverable:** `camel lint` renders ariadne diagnostics and injects the production catalog
  (non-empty: `timer` known); `tests/lint_corpus.rs` discovers route files by glob, runs the
  engine, compares against `tests/fixtures/lint-corpus-baseline.ron`; `Document::apply_fix`
  + re-lint exercised by a test.
- **Exit-criteria:** corpus integration test green (zero diagnostics outside baseline); a test
  asserts the production catalog reports an invalid `timer` option (proving non-empty);
  `cargo build --workspace` green.

## Alternatives considered

- **Reuse `camel-dsl::compile` for parsing.** Rejected: `camel-dsl` depends on
  `camel-core`/`camel-processor`/`camel-auth`/`camel-endpoint`, which would pull the entire
  runtime into `camel-lint` and, transitively, into the future LSP. The compiler's AST also
  uses bare `String` for URIs (no spans) and its `validate_route`/`validate_step` are private.
  Lint parsing the CST directly keeps the dep graph clean and yields byte-exact spans.
- **Approximate spans (line ranges).** Rejected: a linter that points at the wrong location is
  not adopted. noyalib's CST makes byte-exact spans cheap.
- **R-COMBO + scheme-level deprecation in-scope.** Rejected: `ComponentMetadata`/`UriOption`
  expose no fields to express option combinations or scheme deprecation. Shipping a rule
  against absent metadata would be aspirational. Deferred to a bd follow-up that first extends
  the metadata, then adds the rule.
- **`builtin_catalog()` inside `camel-lint`.** Rejected: it would require `camel-lint` to
  depend on `camel-core` (for `Registry` and the live component constructors), breaking the
  boundary. The catalog is constructed in `camel-cli` and injected.
- **Static `StaticMetadataCatalog` from `Config::metadata()` calls (Option B).** Rejected:
  `Component::metadata()` defaults to `ComponentMetadata::minimal(scheme)` and is harvested for
  every registered component, so runtime registration surfaces *all* schemes (rich where opted
  in, minimal otherwise). Static `Config::metadata()` calls cover only the ~8 opted-in structs —
  a strict coverage regression — while adding no dep-graph benefit (`camel-cli` already links
  every component crate). Promoting the test-only `MockCatalog` to a public
  `StaticMetadataCatalog` remains useful as an engine test double but is not the production path.
- **Shared `register_builtin_components(ctx)` used by both `run` and `lint`.** Rejected: `run`'s
  registration is lifecycle-entangled (bridge/pool/datasource/wasm/exec handles captured for
  shutdown and config wiring). A shared function cannot cleanly serve both without forcing
  `lint` to fabricate and drop runtime deps. A separate `register_builtin_components_for_lint`
  keeps each call site honest; list unification is a bd follow-up.
- **Embedding the schema via workspace-root `include_str!`.** Rejected: the file is outside the
  crate and absent in an installed binary. A checked-in copy inside `camel-lint` with an xtask
  byte-equality gate is packaging-safe and drift-proof.
- **Static catalog via `inventory`/`linkme`.** Rejected: dead-code elimination can drop
  constructors silently, producing an empty catalog, which would make every rule inert.
  Explicit registration is reliable; static enumeration is explored in a bd follow-up.
- **Single crate bundling LSP.** Rejected: LSP is additive; `tower-lsp`'s async runtime differs
  from the engine's analysis loop. Splitting lets the engine ship and stabilize before LSP
  (Change B) layers on. LSP is a thin adapter calling the same engine, so the split is
  reversible.
- **Wrapping the whole AST in `Spanned<T>`.** Rejected: annotation noise. Only leaves a rule
  reports on (URI strings, option keys/values) carry `Spanned`.
