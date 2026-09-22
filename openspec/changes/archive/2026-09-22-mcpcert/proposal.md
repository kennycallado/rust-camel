# Proposal: mcpcert — reject blank MCP TLS paths in ROUTE_SCHEMA

## Why

Bd: rc-n3t73 (P2, retro520 finding — lint/runtime contract drift).

The ROUTE_SCHEMA lint accepts any string for `mcp[].server.tls.cert_path`
and `key_path`, while runtime deserialization (camel-dsl `non_empty_path`)
trims and rejects empty or whitespace-only values with
`"<field> must not be empty"`. `camel lint` therefore approves
configuration that boot rejects — the exact "values valid at lint but
rejected at runtime" drift class from the retro520 cross-family audit.

## What Changes

- Add a `pattern` (`\S`) constraint to both TLS path fields in the
  GENERATED route schema, sourced from the schemars derive on
  `RouteDslMcpTlsConfig` (camel-dsl), so the constraint is born next to
  the `non_empty_path` deserializers it mirrors.
- Regenerate `schemas/dsl/route-schema.json` and the byte-synced
  `crates/camel-lint/schema/route-schema.json` copy via `cargo xtask schema`.
- R-SCHEMA then rejects blank/whitespace-only cert/key paths with an
  Error diagnostic naming the field (jsonschema `pattern` keyword,
  instance path `/mcp/0/server/tls/<field>`).
- Unit tests in `crates/camel-lint/src/rules/rschema/tests.rs`: empty and
  whitespace-only cases for BOTH fields, anchored on the offending value.
- Corpus: new negative sibling fixture
  `crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-blank-paths.yaml`
  (extends the rc-ysvjl mcp corpus coverage started by 39fd3bc5) plus a
  justified baseline entry — the fixture MUST fail the gate's emitted-set
  equality, pinning the rejection end to end.

## Sibling audit (rest/https)

ROUTE_SCHEMA carries exactly ONE TLS block: `RouteDslMcpTlsConfig`. The
`rest` DSL block has no TLS fields; https TLS lives in component URI
options/TOML, outside ROUTE_SCHEMA. No sibling fix required — the design
doc records this finding.

## Acceptance Criteria

1. Empty-string and whitespace-only `cert_path`/`key_path` values in a
   `mcp:` block produce R-SCHEMA Errors naming the field.
2. Valid paths (incl. paths with leading/trailing spaces around a real
   path — runtime trims and accepts) stay silent.
3. `cargo xtask schema --check` passes (no drift between generated and
   committed schemas).
4. Corpus gate green with the new fixture + baseline entry.

## Affected Crates

- `camel-dsl` (schemars attribute on `RouteDslMcpTlsConfig`)
- `camel-lint` (regenerated embedded schema; R-SCHEMA unit tests)
- `camel-cli` (corpus fixture + baseline)

## Risk Budget

Low. One generated-schema constraint, additive at the lint surface;
runtime behavior untouched. Residual risk: the `\S` pattern must accept
trimmed-but-valid inputs (it does — it matches "contains at least one
non-whitespace char", exactly `!trim().is_empty()`).

## Self-grill record

**Questions generated:**
1. [glossary] Does the proposal's "GENERATED route schema" +
   "byte-synced copy" language match the canon in the route-lint spec, so
   the single-source constraint is honored?
2. [sharpen] "Add a `pattern` (`\S`) constraint … sourced from the
   schemars derive" — is the derive attribute spelled correctly, or does
   the What-Changes bullet imply a non-compiling attribute?
3. [scenario] Acceptance #2 claims paths with leading/trailing spaces
   around a real path stay silent. Does `\S` actually pass
   `" /etc/certs/crm.pem "`?
4. [cross-ref] Does the existing clean fixture `mixed-routes-rest-mcp.yaml`
   stay clean after the constraint lands (its paths must contain `\S`)?

**Answers (with citations):**
1. [glossary] Matches. The route-lint spec's "Schema asset is embedded and
   kept byte-equal" canon is honored: proposal regenerates both
   `schemas/dsl/route-schema.json` and the byte-equal
   `crates/camel-lint/schema/route-schema.json` via `cargo xtask schema`
   (`rschema.rs` compiles the embedded `ROUTE_SCHEMA` via `include_str!`;
   the `schema --check` gate enforces byte-equality). No hand-editing.
2. [sharpen] The What-Changes bullet ("`pattern` (`\S`) … sourced from the
   schemars derive") is directionally correct but design.md carried the
   wrong attribute spelling (bare `regex = "\\S"`). Corrected in design.md
   to `schemars(regex(pattern = r"\S"))`. Proposal prose needs no change —
   it does not pin the attribute syntax. → tracked as design FIX 1.
3. [scenario] Yes. Harness result: `" /etc/certs/crm.pem "` →
   `fancy.is_match == true` (unanchored `\S` finds the non-ws path chars) →
   no diagnostic; boot trims to a valid path and accepts. Parity holds.
4. [cross-ref] Stays clean. `mixed-routes-rest-mcp.yaml` uses
   `/etc/certs/crm.pem` and `/etc/certs/crm-key.pem`
   (`camel-dsl/src/mcp.rs:401-402` mirror the same literals); both contain
   `\S`, so the new `pattern` cannot flag them. It remains a
   zero-false-positives witness (no baseline entry).

**Outcome:** confirm — proposal is coherent and consistent with the
route-lint canon; the only defect lives in design.md's attribute spelling
(design FIX 1), which does not alter the proposal's What/Why/Acceptance.
**Self-grill mode:** self-grill-proposals skill (e_opus for e_gpt;
cross-family verification satisfied).
