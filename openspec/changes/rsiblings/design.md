# Design: rsiblings

## Approach

**Chosen fix option: (a) — append the non-pattern nested errors as
leaf diagnostics too.** Rejected (b) (keep the collapsed parent
alongside the pattern leaves).

Defense of (a):

1. **Lint-boot parity.** Both co-located defects are real boot
   failures (`non_empty_path` rejects the blank path;
   `deny_unknown_fields` rejects the unknown key; a non-string
   `key_path` fails deserialization). rc-n3t73's founding bug class
   was "lint approves what boot rejects"; reporting both defects in
   one lint pass closes the gap instead of forcing a
   fix-re-lint-fix cycle.
2. **De-collapse philosophy.** The arm exists so R-SCHEMA "anchors on
   the offending value" (module comment). Option (b) re-introduces
   the collapsed whole-instance echo — the exact noise shape the arm
   was built to remove — and anchors it on the container, not the
   defect.
3. **Byte-exactness.** Sibling diagnostics render through the
   existing `diagnostic_message(nested)`: non-AnyOf kinds keep
   jsonschema `Display` verbatim (scalar/single-key echoes cannot
   reorder); a nested AnyOf-kind sibling (double-collapse shape)
   routes through `canonical_json` — deterministic. No new rendering
   path, no drift on pinned classes.

Mechanics (pass 2 of the `AnyOf` arm, after `pattern_errors`
collection):

- Second collection walk over the same `context` branches:
  every nested error whose kind is NOT `Pattern` is a sibling
  candidate.
- **Noise exclusion**: a `Type`-kind error whose instance path equals
  the collapsed anyOf node's own path is skipped — that is the
  `{"type": "null"}` sibling branch failing *because* the instance is
  an object of the intended shape, not a defect. (Branch 1 is an
  object schema; an object instance cannot fail it with an own-node
  `Type` error, so this exclusion cannot swallow a real defect.)
- Depth: strictly-deeper siblings are always real leaf defects
  (e.g. `key_path: []` under a tls-level or mcp-level collapse).
  Equal-depth non-`Type` siblings are real container-anchored defects
  (`additionalProperties` at the anyOf node itself, `required`).
- Dedup by `(instance_path, diagnostic_message)` preserving
  first-occurrence order (branches retry the same subschema shapes).
- Emission order: pattern leaves first (unchanged existing order),
  then siblings in first-occurrence order.
- Anchoring mirrors the existing arms: `additionalProperties`
  siblings expand one diagnostic per unexpected key via
  `key_span_for(parent + key)`; every other sibling anchors via
  `value_span_for` on its instance path (same as the `_` arm).
- Siblings are appended ONLY when `pattern_errors` is non-empty —
  pure-non-pattern collapses keep today's single collapsed
  diagnostic byte-identically (pinned by
  `rschema_exception_disposition_oneof_unchanged`,
  `rschema_rest_binding_oneof_unchanged`). Pure-pattern cases gain
  zero siblings (only the excluded null-branch noise exists), so the
  existing blank-path tests stay byte-identical.
- Pass 1 (permission targeted) precedence is untouched: it still
  `continue`s before pass 2.

The KNOWN LIMITATION (pattern pass) comment is rewritten to describe
the new behavior; the permission-pass limitation comment stays.

Empirical guard: tests pin the actual jsonschema 0.52.1 branch shapes
(pattern + additionalProperties; pattern + type). If a probe reveals
a different branch shape than designed, the fix adapts the exclusion
rule, never the pinned single-defect classes.

## Affected crates

- camel-lint: `src/rules/rschema.rs` (pass-2 sibling append + comment
  rewrite); `src/rules/rschema/tests.rs` (co-occurrence tests,
  pure-non-pattern pin).
- camel-cli: corpus fixture `mcp-tls-sibling-defects.yaml` +
  baseline entry (test fixtures only, no product code).

## Architecture boundaries

Hexagonal stance unchanged: the rule is lint-side (adapters/rules
periphery), reads the committed ROUTE_SCHEMA, emits Diagnostics.
No DSL, runtime, or schema change; `canonical_json`/
`diagnostic_message` are consumed as-is. R-SCHEMA remains a
read-only analysis rule (no side effects, hermetic).

## Phases

Single-phase: one coherent rule change + tests + fixture, no
milestone grouping.
