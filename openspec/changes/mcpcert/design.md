# Design: mcpcert — reject blank MCP TLS paths in ROUTE_SCHEMA

## Context

- Runtime contract: `crates/camel-dsl/src/mcp.rs` `non_empty_path`
  deserializes `String`, trims, errors `"<field> must not be empty"` on
  empty-after-trim. Applied to `RouteDslMcpTlsConfig.cert_path` and
  `.key_path`.
- Lint contract: `crates/camel-lint/src/rules/rschema.rs` validates the
  interpolated document against `ROUTE_SCHEMA`
  (`crates/camel-lint/schema/route-schema.json`, embedded via
  `include_str!`). The schema's `$defs/RouteDslMcpTlsConfig` types both
  fields as bare `{"type": "string"}` — blank strings pass lint, fail boot.
- Schema generation: `cargo xtask schema` (scripts/xtask/src/main.rs)
  runs `schemars::schema_for!(RouteDslSchemaEnvelope)` (schemars 1.2.2),
  writes `schemas/dsl/route-schema.json`, and byte-syncs the camel-lint
  embedded copy. `schema --check` fails on drift.

## Chosen approach: schemars `regex` attribute (source-generated pattern)

Add to both fields of `RouteDslMcpTlsConfig`:

```rust
#[cfg_attr(feature = "schema", schemars(regex(pattern = r"\S")))]
```

NOTE (verified against schemars_derive 1.2.2): the attribute MUST use the
nested `regex(pattern = ...)` form (or the `pattern = ...` name-value form
`schemars(pattern = r"\S")`). The bare `schemars(regex = "\\S")`
name-value form does NOT compile — `parse_schemars_regex`
(`schemars_derive-1.2.2/src/attr/parse_meta.rs:150`) requires the nested
`pattern` key. Using a raw string literal `r"\S"` keeps the backslash
literal without double-escaping.

schemars 1.2.2 then emits `insert_validation_property(schema, "string",
"pattern", (r"\S").to_string())` → `"pattern": "\\S"` in the generated
JSON for each field (`schemars_derive-1.2.2/src/attr/validation.rs`). The
jsonschema 0.52.1 validator compiles bare `\S` via fancy-regex (its
default engine) and matches it as an UNANCHORED search
(`is_match`, `keywords/pattern.rs`) — the `NoWhitespace` fast-path only
fires for the exact sentinel `^\S*$` (`src/regex.rs`), not bare `\S`.
fancy-regex/regex treat `\S` as the exact complement of Unicode
`White_Space`, which equals Rust `trim()`'s set (empirically: 0 divergence
across all 1.1M Unicode scalar values). Therefore:

- `""` → fails pattern → R-SCHEMA Error (parity: runtime rejects).
- `"   "` → fails pattern → Error (parity).
- `" /etc/certs/a.pem "` → passes pattern → silent (parity: runtime
  trims to a valid path and accepts).
- `${env:CERT:-}` whole-scalar empty default → substitutes to `""` →
  Error (parity: boot substitutes then rejects). The rc-93wct typing
  mirror keeps substituted leaves as strings, so no interaction with the
  integer carve-out (string position).

The jsonschema `pattern` keyword on a field nested under an `anyOf`
(e.g. `tls: anyOf [TlsConfig, null]` — schemars' Option shape) COLLAPSES
in jsonschema 0.52.1: `iter_errors` yields one `AnyOf` error anchored at
the anyOf node (`/mcp/0/server/tls`) with the message "not valid under
any of the schemas", leaving the leaf Pattern error buried in
`ValidationErrorKind::AnyOf { context }` (Vec per branch of nested
owned ValidationErrors). The collapsed diagnostic names neither field
nor defect — failing the mission's "diagnostic naming the field".

R-SCHEMA therefore gains ONE surgical arm in its error loop
(`crates/camel-lint/src/rules/rschema.rs`): when the kind is
`ValidationErrorKind::AnyOf { context }`, surface nested `Pattern`
errors whose `instance_path` is strictly deeper than the anyOf node's —
each becomes its own leaf-anchored diagnostic (message
`{instance} does not match "{pattern}"`). When one or more surface,
they REPLACE the collapsed anyOf diagnostic for that node; when none
surface, today's collapsed behavior is kept byte-identical (existing
anyOf collapses for type/required/enum defects — e.g. the route-lint
spec scenario "anyOf failure reports the value" — are untouched: only
`Pattern` de-collapses, and the schema's ONLY pattern keywords after
this change are the two TLS path fields). Dedup surfaced errors by
(instance_path, pattern) pair — an anyOf branch context can repeat a
leaf error across branches.

Alternatives rejected:
- Hand-editing the JSON files — forbidden: both copies are generated;
  `schema --check` would fail on the next regeneration (single-source).
- `minLength: 1` (schemars `length(min = 1)`) — rejects `""` but accepts
  whitespace-only; drift class would persist.
- Newtype with custom `JsonSchema` impl — more surface for one keyword;
  regex attr is the schemars-native way.

## Sibling TLS audit (mission: "rest/https")

Walked the committed schema: the ONLY TLS block under the envelope is
`$defs/RouteDslMcpServer/properties/tls` → `RouteDslMcpTlsConfig`.
`rest:` blocks carry host/port/path/operations (no TLS); https TLS is
component-level (camel-http URI options), outside ROUTE_SCHEMA. Other
path-ish fields (`path`, `jsonpath`, `xpath`, `key_prefix`) are not
cert-path class (no runtime trim-reject twin — different semantics).
Conclusion: MCP is the only instance; fix is uniform by construction.

## Files

1. `crates/camel-dsl/src/mcp.rs` — two `cfg_attr` lines on
   `RouteDslMcpTlsConfig` fields (schema feature only; no runtime code).
2. `schemas/dsl/route-schema.json` + `crates/camel-lint/schema/route-schema.json`
   — regenerated via `cargo xtask schema` (from the worktree).
3. `crates/camel-lint/src/rules/rschema.rs` — the AnyOf de-collapse arm
   described above (one match arm + helper; Pattern-only, strictly
   deeper paths, replace-when-present, dedup by (path, pattern)).
4. `crates/camel-lint/src/rules/rschema/tests.rs` — 6 new unit tests
   (4 blank cases: cert/key × empty/whitespace; 1 padded-valid control;
   1 empty-env-default case), anchored on the offending value span.
5. `crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-blank-paths.yaml`
   — negative sibling of `mixed-routes-rest-mcp.yaml` (39fd3bc5): same
   mcp block shape, `cert_path: ""` + `key_path: "  "` → two R-SCHEMA
   Errors (baseline collapses to one `("R-SCHEMA", "error")` entry).
6. `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` — entry
   with justification comment.

## Boundaries & ADRs

Data-plane/runtime untouched; change lives in the DSL schema surface
(control-plane config contract). Relevant: ADR-0060 (MCP first-class
component — mcp block ownership), lint/runtime parity canon from
`route-lint` spec ("Schema asset is embedded and kept byte-equal").
`schemas/ts/*` output unchanged (regex attr does not affect ts_rs).

## Gates

fmt; `clippy -p camel-lint -D warnings`; camel-lint unit tests;
`lint_corpus` gate (camel-cli test `corpus_zero_false_positives`); plus
`cargo xtask schema --check` to prove no drift. Run in the worktree
only.

## Phases

Single-phase: schema constraint + tests + corpus fixture land together
(the corpus fixture fails until the schema rejects blanks — one
coherent unit, ~60 lines).

## Self-grill record

**Questions generated:**
1. [glossary] Is "cert-path class" a canonical term, and does the schema
   truly carry exactly one TLS block (no sibling drift twin)?
2. [sharpen] Does `schemars(regex = "\\S")` (bare name-value) compile
   under schemars_derive 1.2.2, or is the attribute syntax wrong?
3. [scenario] Does `\S` under the jsonschema pattern validator equal
   `!trim().is_empty()` on ALL Unicode whitespace (NBSP, ZWSP, BOM,
   Mongolian vowel separator), or is there a divergence codepoint?
4. [cross-ref] Does jsonschema 0.52.1 treat bare `\S` as an unanchored
   search (contains-a-non-ws-char) rather than a full match, and does
   `analyze_pattern` short-circuit it to the `^\S*$` NoWhitespace
   optimization?

**Answers (with citations):**
1. [glossary] Confirmed. The committed schema's only TLS ref is
   `$defs/RouteDslMcpServer/properties/tls -> RouteDslMcpTlsConfig`
   (`crates/camel-lint/schema/route-schema.json:1693,1711`). `rest` blocks
   carry host/port/path/operations only (`camel-dsl/src/openapi.rs`,
   `RouteDslRest` — no TLS). No other field pairs a runtime trim-reject
   deserializer, so "cert-path class" is a precise one-instance set. The
   sibling audit stands.
2. [sharpen] WRONG. schemars_derive 1.2.2 requires nested syntax:
   `parse_schemars_regex` (`schemars_derive-1.2.2/src/attr/parse_meta.rs:150-174`)
   demands `regex(pattern = ...)`; the bare `regex = "..."` name-value form
   is not accepted. The canonical forms are `schemars(regex(pattern = r"\S"))`
   or `schemars(pattern = r"\S")` (`parse_meta.rs:146`, the `pattern`
   name-value branch). Both feed `insert_validation_property(schema,
   "string", "pattern", (#expr).to_string())`
   (`schemars_derive-1.2.2/src/attr/validation.rs`), emitting
   `"pattern": "\\S"`. The pattern-emission claim is correct; only the
   attribute spelling is wrong. → REQUIRED FIX 1.
3. [scenario] Verified EMPTY divergence set. A standalone harness compiled
   bare `\S` with fancy-regex 0.19 (jsonschema's default engine) and the
   regex crate 1.13, then compared `is_match` against `str::trim().is_empty()`
   across all 1,112,064 Unicode scalar values: 0 divergences. NBSP (U+00A0)
   and every Unicode space are non-matches under both (whitespace-only →
   rejected, boot parity); ZWSP/BOM/Mongolian (not White_Space) match as
   non-ws under both (accepted, boot parity). `\S` is the exact complement
   of Rust `trim()`. Parity claim confirmed.
4. [cross-ref] Confirmed. `PatternValidator::validate` calls
   `self.regex.is_match(&item)` — unanchored contains-search
   (`jsonschema-0.52.1/src/keywords/pattern.rs`). `analyze_pattern` only
   emits `NoWhitespace` for the exact sentinel `^\S*$`
   (`jsonschema-0.52.1/src/regex.rs:187` test), NOT bare `\S`, so bare `\S`
   falls through to the full fancy-regex engine — the path the scenario
   harness exercised. Unanchored `\S` = "≥1 non-ws char" = `!trim().is_empty()`.

**Outcome:** refine — the approach is sound; one attribute-syntax defect
(REQUIRED FIX 1) must be corrected before planning. The pattern-emission,
parity, sibling-audit, and unanchored-search claims all hold under
cross-family verification.
**Self-grill mode:** self-grill-proposals skill (e_opus standing in for
e_gpt; cross-family verification of GLM-generated spec satisfied).
