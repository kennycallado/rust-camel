# Design: env-driven-knobs — unit-tier document env layer

## Context

- ADR-0069 §4 (Environment parity and hermeticity): the layered env source is
  "document `env` first, allowlisted ambient second, defaults third" and is "an input
  to the DSL and config loaders, not a process-global rewrite". The unit tier takes
  the STRICTEST slice: document env only.
- Canon: camel-dsl `env_interpolation.rs` — substituted leaves keep string typing
  (deliberate; wave-E rc-93wct). Boot path rejects int-typed placeholders; the unit
  tier MUST keep boot parity (it is a correctness invariant, not just canon text).
- rc-4hexo (b47b6b36, landed): default-only parity across the three unit-tier seams,
  `EnvUnresolved { var, field }` exit-2 semantics for no-default identifiers.
- Papal verdict (e_opus, ses_f7d7b982effeGHRiWlWUMoBRte): PROCEED-WITH-CHANGES,
  option C1 pure string layer. Typed-leaf substitution (C2) was DISQUALIFIED: it
  would let the unit tier resolve int positions the boot path itself rejects,
  splitting boot↔test behavior.

## Goals / Non-goals

**Goals**

1. Test authors steer string-typed interpolation per-document, hermetically
   (route files, inline routes, and doc-side identifiers — one mechanism).
2. Zero camel-dsl changes; zero DSL contract changes; zero scenario-tier changes.
3. Boot parity preserved exactly: typing semantics, error wording, escape forms.

**Non-goals**

1. Numeric/boolean knob typing (`open_duration_ms: u64` stays unserved → rc-v1sw).
2. Ambient passthrough in the unit tier (scenario-tier-only concern).
3. Recursive interpolation, key-grammar validation, cross-doc inheritance.

## Decisions

### D1. `env:` map shape — `BTreeMap<String,String>`, serde(default), opt-in

`TestDocument` gains `#[serde(default)] pub env: BTreeMap<String,String>`.
`deny_unknown_fields` keeps old documents working (absent key = empty map).
Non-string VALUES (YAML number/bool/null) fail serde deserialization → existing
document-error path. This is deliberate: class-A knobs are not served, so typed
fixture values would be a lie (an int-looking `CB_MS: 500` would still fail at the
u64 position after string substitution — rejecting it at the doc boundary is
honest and catches the misunderstanding early with a clearer error).

### D2. Lookup resolution — doc-env first, then default, never ambient

Resolution order per placeholder: document `env:` map → inline default →
`EnvUnresolved`/load error. No ambient read, no passthrough, no harness layer.
The closure is local: `&|name| doc.env.get(name).cloned()`. The `LayeredEnv` type
is NOT imported into the unit tier — it carries harness-provisioned + passthrough +
ambient machinery this tier must not expose (papal ruling).

### D3. Three seams, existing signatures

1. `runner.rs` routeFiles/routeFilesFromRoot: `camel_dsl::load_from_file(&full)` →
   `camel_dsl::load_from_file_with_env(&full, &lookup)`.
2. `runner.rs` inline routes: existing `interpolate_yaml_source(&text, &|_| None)`
   → pass `&lookup`.
3. `document.rs` identifier pass: `interpolate_identifier` gains a
   `lookup: &dyn Fn(&str) -> Option<String>` parameter; the caller
   (`interpolate_identifier_fields`) builds it from the doc's own `env` map.
   Ordering is safe: identifier interpolation runs after deserialization, so
   `doc.env` is already populated; `env:` keys/values are themselves never
   interpolated (no recursive resolution — values substitute verbatim).

### D4. Error semantics unchanged

No-default placeholder not in the doc env → existing `TestDocError::EnvUnresolved
{ var, field }` exit 2 (route sources keep their loader wording). Duplicate-key and
verbatim-match guards keep operating on RESOLVED values (b47b6b36 behavior, only
the closure body changes).

### D5. Considered and rejected

- **envPassthrough allowlist:** rejected (papal) — unit tier stays doc-strict;
  ADR §4 ambient-off default taken to its maximum.
- **LayeredEnv import:** rejected (papal) — over-scoped type for this tier.
- **Key-grammar validation on `env:` keys:** rejected — a typo'd key never matches
  the scanner grammar, so the steering silently no-ops and the test fails visibly
  with the default value; an extra validator adds surface for no hermeticity gain.
- **Typed env values (C2):** disqualified — boot-parity split (papal correction 1).
- **Recursive interpolation of env values:** rejected — fixture values are data,
  not templates; `${env:A}` whose value contains `${env:B}` substitutes the literal
  text.

### D6. Spec-key renames

The two rc-4hexo-era requirement names contain "default-only", which becomes false.
Both are RENAMED (openspec delta op) with full MODIFIED bodies; scenario coverage
carried over and extended.

## Risks / Trade-offs

- **Misuse expectation:** authors may expect `env: CB_MS: "500"` to serve
  `open_duration_ms`. Mitigated by D1's early string-only rejection at the doc
  boundary + the spec scenario that pins int-position failure WITH a doc value
  present + user docs pointing numeric needs at the rc-v1sw track.
- **Identifier/route divergence:** both sides consult the SAME map through the SAME
  scanner grammar, so name-match parity is preserved by construction (the rc-4hexo
  invariant generalizes from "both take defaults" to "both take doc-env-then-defaults").
- **Boot parity:** no camel-dsl code is touched; typing semantics are inherited
  from the untouched scanner — parity cannot regress by construction.

## Migration Plan

Backward compatible: docs without `env:` behave exactly as before (empty map =
default-only). No deprecation, no flags.

## Open Questions

None — papal adjudicated all decision points.
