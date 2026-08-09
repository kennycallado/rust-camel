# Proposal: consolidate-uri-metadata

## Why

Six components (sql, file, cron, opensearch, ws, container) annotate their endpoint
config structs with `#[uri_param]` for URI parsing, but never override
`Component::metadata()`. They fall back to `ComponentMetadata::minimal(scheme)` with an
empty `uri_options` list, making every parameter invisible to the
`ComponentMetadataCatalog`. A second group (timer, http) declares parameters twice —
once in `#[uri_param]` and again by hand in `fn metadata()` — inviting silent drift.

The root cause is that the `#[derive(UriConfig)]` macro (`camel-endpoint-macros`) only
parses `name` and `default` from `#[uri_param]` and only generates `from_uri` parsing.
It cannot express `secret`, `deprecated`, `aliases`, `required`, or `kind`, and it
generates no metadata. Two parallel authoring paths with no single source of truth
produce the gap (6 invisible) and the duplication (2 drifted) seen across the codebase.

This blocks the downstream lint consumer (rc-5tm3: `get_metadata("sql").uri_options` is
empty, so every sql param is a false "unknown") and the `parameters:` DSL feature
(rc-6vni: the `secret` flag needed for redaction exists only on http).

Bd issue: rc-4cos.

## What Changes

**Full unification — no backward-compatible dual-path.** Every component in the project
is converted to the macro-derived single source of truth. No hand-written
`fn metadata()` UriOption lists survive.

**In scope:**
- Extend `#[uri_param]` accepted keys: `desc`, `required`, `secret`, `deprecated`,
  `aliases`, `kind` (override). Existing `name`/`default` keys unchanged.
- Add `OptionKind` inference from the Rust field type (Duration, bool, int, float,
  String, Vec) with the rule that inference never emits `Enum` (explicit `kind` only).
- Generate a `fn uri_options() -> Vec<UriOption>` helper on every `#[derive(UriConfig)]`
  struct.
- Add `#[uri_config(metadata(scheme, description, capabilities))]` that generates a
  `Component::metadata()` override populating `uri_options`.
- Add `ComponentMetadata` builder methods (`with_description`, `with_capabilities`,
  `with_uri_options`) to `camel-api` (currently absent — only `minimal()` exists).
- **Migrate ALL 12 components** to the unified path:
  - 6 MACRO-ONLY (sql, file, cron, opensearch, ws, container): opt into metadata gen.
  - timer (already derives UriConfig): remove hand-written metadata, use opt-in + compose.
  - http (3 manual `impl UriConfig` structs with bespoke parsing): use
    `#[uri_config(skip_impl)]` — metadata derives from the macro, manual parse logic is
    **retained** byte-stable. Auth fields get `secret` attrs. Log uses derive or skip_impl.
  - direct, mock, seda (no UriConfig, hand-written `from_uri`): adopt
    `#[derive(UriConfig)]` + `#[uri_param]` + opt-in metadata. Mock is trivial (zero params).
- Compile-error guardrail: a param that is both `secret` and has a `default` is rejected.
- Amend ADR-0041 (derivation mechanism) + CONTEXT-MAP glossary.

**Out of scope:**
- `#[non_exhaustive]` on `UriOption`/`ComponentMetadata` (separate ADR-0049 hygiene).
- JSON-Schema xtask generation (ADR-0041 already defers this).
- The lint consumer (rc-5tm3) and `parameters:` DSL (rc-6vni) — separate downstream
  changes filed with `depends-on: consolidate-uri-metadata`.

## Acceptance criteria

- ALL 12 components return non-empty `uri_options` from
  `ComponentMetadataCatalog::get_metadata(scheme)` (except trivial components with
  genuinely zero params, if any).
- NO component retains a hand-written `fn metadata()` with a `UriOption` list — all
  metadata derives from the macro.
- All existing `from_uri` parse tests pass byte-stable (purely additive codegen for
  derive-using structs; structural-equivalent for manual-impl conversions).
- No scheme has duplicate option names in `uri_options` (dedup invariant).
- `secret` + `default` on the same param is a compile error.
- Inference never emits `OptionKind::Enum` (only explicit `kind = "enum:.."`).

## Risk budget

- Macro codegen changes are purely additive (`uri_options()` + optional `metadata()`); the
  existing `from_uri` token stream must not change. This is the primary regression risk
  and is gated by byte-stable parse-test verification.
- Component migrations are incremental (one per commit); a half-migrated workspace is
  valid at every commit.
- No breaking change to the `Component` trait, `camel-component-api`, or any existing
  component's parse behavior.
