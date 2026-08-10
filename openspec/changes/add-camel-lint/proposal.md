# Proposal: add-camel-lint

## Why

Framework users author YAML/JSON route files with **no author-time feedback**. URI typos,
unknown parameters, literal secrets, and deprecated options surface only at runtime. The
DSL compiler (`camel-dsl::compile`) validates structure but never consults
`ComponentMetadataCatalog`, so semantic mistakes (unknown option, wrong option kind,
deprecated option, literal secret) pass silently. The 2026-08-09 landing of
`consolidate-uri-metadata` (ADR-0041) made full component metadata (schemes, `uri_options`,
kinds, secret/deprecated flags, aliases) available behind the `ComponentMetadataCatalog`
trait in `camel-api` — but no consumer of it exists yet. This change builds that consumer:
a standalone lint engine plus a `camel lint` CLI that reports route diagnostics with
**exact source spans** and zero false positives over the in-tree corpus.

Bd: rc-g2rz (Change A). Change B (camel-lsp, rc-o5qz) is additive and blocked on this.

## What Changes

**Included**
- New crate `camel-lint`: a span-exact rule engine that parses YAML/JSON itself via the
  `noyalib` CST (it does **not** depend on `camel-dsl` or `camel-core`) and validates routes
  against an injected `Arc<dyn ComponentMetadataCatalog>`.
- Five rules (each implementable with the metadata fields that exist today):
  - **R-SYN** — syntax errors from the noyalib parser, byte-exact.
  - **R-SCHEMA** — `jsonschema` validation against the checked-in `route-schema.json`, with
    per-keyword span anchoring.
  - **R-URI-known** — for schemes the catalog knows: unknown option, wrong option kind,
    missing required option. For schemes the catalog does **not** know: a single
    informational `unverified-scheme` note, no option diagnostics.
  - **R-SECRET** — a secret option set to a literal value (not a `${...}` / `{{...}}`
    interpolation reference) emits a warning.
  - **R-DEPRECATED** — an option whose catalog metadata carries a `deprecated` message
    emits a warning.
- `camel lint` CLI subcommand in `camel-cli` (which already depends on `camel-core`):
  builds the production catalog via a lint-specific
  `register_builtin_components_for_lint(ctx)` (NOT shared with `run`, whose registration is
  lifecycle-entangled), wraps the registry in `RuntimeComponentMetadataCatalog`, injects it
  into `LintEngine`, renders ariadne diagnostics, exits 0/1/2 by severity.
- **Zero-false-positives merge gate**: an integration test in `camel-cli` runs the engine
  over every route file discovered by a glob rule and compares emitted diagnostics against a
  checked-in baseline; any diagnostic outside the baseline fails the test.

**Excluded** (explicitly)
- **R-COMBO** (illegal option combinations) and **scheme-level deprecation** — deferred to a
  bd follow-up because `ComponentMetadata`/`UriOption` currently expose no fields to express
  option combinations or scheme deprecation. They land after a metadata extension change.
- LSP server (Change B, rc-o5qz).
- New xtask lints for the in-tree source (separate concern; this is a user-facing tool).
- Static catalog enumeration via `inventory`/`linkme` (DCE footgun → bd follow-up).

## Acceptance criteria

- The `camel lint` corpus integration test passes: zero diagnostics outside the checked-in
  baseline over the discovered in-tree route files.
- All five rules active, each with at least one executable GIVEN/WHEN/THEN test asserting
  byte-exact spans.
- `camel-lint` depends only on `camel-api`, `noyalib`, `jsonschema`, `ariadne`, `serde`,
  `serde_json`, and `thiserror`
  — never on `camel-dsl`, `camel-core`, or `camel-cli`. (Boundary verified by the workspace
  hexagonal-architecture test.)
- The production catalog is constructed in `camel-cli` and injected; `camel-lint` exposes no
  `builtin_catalog()` and no dependency on `Registry`.
- Engine is incremental-ready: the engine is stateless (`lint(&self, source)` retains no
  document), and `Document::apply_fix(&mut self, &Fix)` re-parses the affected region via
  `cst::Document::replace_span`; the caller re-runs `engine.lint(&document.raw)` for refreshed
  diagnostics. Diagnostics are protocol-agnostic with stable codes, and partial/malformed input
  does not crash the engine (parse failures flow through `Document.parse_failure` to R-SYN).
- Exit codes: 0 clean, 1 any error-severity diagnostic, 2 engine/CLI misuse.

## Risk budget

- **Zero false positives is non-negotiable** and is the merge gate. A rule that cannot meet
  it on the corpus is gated (the `unverified-scheme` guard for R-URI-known) or cut — never
  shipped noisy.
- **No external infra dependency** in tests (no Kafka/Redis/Docker) — lint is pure analysis.
- **Boundary integrity is a merge gate.** `camel-lint` reaching into `camel-core` or
  `camel-dsl` blocks merge; the workspace architecture test enforces it.
- **Approximate spans are out of bounds.** A span that points at a line instead of the
  offending token is a bug, not a polish item.
