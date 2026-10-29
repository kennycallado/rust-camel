# Self-grill record — jsonschema-format-enforcement

Mode: self-grill-proposals (expert blessing pass skipped per mission order
450 + PATTERN-V2 MODEL/EXPERT AVAILABILITY 2026-10-27: expert pass optional,
e_gpt only for a genuine ruling; no ruling required — all decisions are
pinned by the order). No speculation: every answer cites files read this
session.

## DP-1 — Compile via `jsonschema::options().should_validate_formats(true).build()`

1. [glossary] Does "format validation enabled" conflict with existing
   glossary/spec terms?
   → No. `openspec/specs/rest-dsl/spec.md:73` ("JSON binding mode
   pipeline") names the unmarshal step carrying `request_schema` but
   defines no schema-validation semantics; CONTEXT-MAP.md:186 describes
   "JSON Schema validation" generically. New domain
   `json-schema-validation` owns the semantics.
2. [sharpen] Is "enforced for every draft" the canonical claim?
   → Yes: jsonschema-0.58.5 `src/options.rs:386` — "Default behavior
   depends on the draft version. This method overrides" — and
   `options.rs:390` sets `validate_formats: Option<bool>` to `Some(yes)`.
   Draft auto-detection is preserved because `ValidationOptions.draft`
   defaults to `None` (`options.rs:27`, `options.rs:62`).
3. [scenario] Edge: `$schema: 2019-09` schema + `format` — forced?
   → Yes, `Some(true)` applies across drafts; this matches the bd rc-giuvl
   note (draft-dependent default exists because 2019-09/2020-12 annotate).
   Spec scenarios cover the two order-required cases (draft-07, absent
   `$schema`).
4. [cross-ref] Do any call sites already use `options()`/formats?
   → No: only `validator_for` at
   `crates/camel-processor/src/json_schema_validate.rs:46`,
   `crates/components/camel-validator/src/compiled.rs:137,176`. Other
   jsonschema consumers (mcp, dsl-openapi, lint) are out of scope per the
   order.

Outcome: confirm.

## DP-2 — `[components.validator] validate-formats` via ComponentBundle

1. [glossary] Does the key shape match config conventions?
   → Yes: `[components.<config_key()>]` registry
   (`crates/camel-bundles/src/lib.rs:206,221`); template precedent
   `config_key() == "template"` (`camel-template/src/bundle.rs:62-107`);
   kebab-case key follows the `default-autoescape` shape.
2. [sharpen] "Restores previous accept behavior" is precise: with the
   opt-out, `format` is annotation-only under `validator_for`, so a
   malformed email validates. Spec scenario states exactly this.
3. [scenario] `validate-formats = "no"` (string) must fail config load
   naming the key — template precedent
   `from_toml_rejects_unknown_default_autoescape`
   (`camel-template/src/bundle.rs:148`). Spec scenario covers it.
4. [cross-ref] Dependency edge? `camel-bundles/Cargo.toml:44` already
   depends on `camel-component-validator` (XSD bridge wiring,
   `camel-bundles/src/lib.rs:51`). Registering the bundle adds no edge.

Outcome: confirm (flag→endpoint plumbing follows the TemplateBundle
`register_all` precedent, `camel-template/src/bundle.rs:82`; exact
injection site is worker-level detail).

## DP-3 — REST `request_schema` stays always-on

1. [glossary] "Per-operation carrier": the REST operation struct exposes
   no config slot beyond schema fields (`camel-dsl/src/rest.rs:534`
   struct literal shows the field set). No carrier exists.
2. [sharpen] "Always-on" is documented in the REST page
   (`docs/src/yaml-dsl/top-level-blocks.md` REST section) and the
   migration note.
3. [scenario] Operator wants old REST behavior: no mitigation exists by
   design; order requirement 2 sanctions exactly this ("keep it always-on
   for now and say so in the docs/park; do not invent a DSL key without a
   spec").
4. [cross-ref] A per-route `[rest]` config section would be a new DSL key
   — forbidden by the order without a spec.

Outcome: confirm.

## DP-4 — No shared policy helper across the two crates

1. [glossary] Order's "shared policy helper" is realized as a private
   `compile_json_schema` per crate; no new public surface.
2. [sharpen] Duplication is ~8 lines per crate vs. a new dependency edge
   (`lint-component-deps` risk the order calls out). Chosen: duplication
   + cross-referencing comments.
3. [scenario] Drift risk (one site loses the flag): mitigated by tests in
   BOTH crates asserting format enforcement (order test matrix).
4. [cross-ref] No common crate exists: camel-processor uses `camel_api`
   (`json_schema_validate.rs:7-8`); camel-validator uses
   `camel_component_api` (`compiled.rs:5`). Hosting a helper in either
   API crate would add an edge.

Outcome: confirm.

## ADR test

Hard to reverse: no (opt-out exists). Surprising: partially (changelog +
docs cover). Real trade-off: modest. Not all three → no new ADR; bd
rc-giuvl note agrees ("ADR only if not configurable" — it is
configurable).
