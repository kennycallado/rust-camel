# Design: env-int-placeholder-typing

## Approach

One algorithm for both arms — **provenance-tracked interpolation + probe
subset search at the typed boundary**, with the real parser/deserializer as
the oracle. No hand-maintained integer tables (a first revision's table was
proven incomplete by the bless expert: `timeout_ms`, `parallel_limit`,
`threshold`, `max_entry_bytes`, and other numeric fields exist across the
model), no toml error-message parsing (format not pinned), no schemars
feature flip, no public type changes.

**Correctness argument (why probing is sound).** A candidate leaf is a
whole-scalar substituted placeholder whose value parses as i64 or u64.
A coerced copy parses successfully only if every integer-typed position
carrying a placeholder candidate is coerced (otherwise the strict typed
field — `usize`, `u64`, … — rejects the string) . So every parsing subset
contains S_min = the set of integer-position candidates. Subsets that
coerce additional candidates may ALSO parse when the extra leaf lands on a
polymorphic position (`SetHeaderStepDef.value` wraps
`serde_json::Value`, which accepts numbers as well as strings) — but the
search enumerates
subsets smallest-first in document order and stops at the first success,
and the only parsing subset of size |S_min| is S_min itself. The search
therefore returns the unique minimal parsing subset: exactly the integer
positions, never a polymorphic superset. String-typed positions (the
mirror case) are structurally excluded from the returned subset. Unsigned
bounds, u32/u16 narrowing, and negative values are enforced by the real
parse, not by heuristics. Mixed-document tests must pin this in BOTH
document orders (integer step first and string step first).

**camel-dsl.** `interpolate_env_tree` gains a provenance variant: besides
the interpolated document it returns the set of tree paths whose authored
scalar was exactly one whole-scalar `${env:...}` token (substituted
leaves). The interpolation seam itself keeps string typing, comments,
escapes, and the legacy fallback unchanged; the fallback path carries no
provenance and never probes. A new public seam
(`parse_routes_with_env(raw, lookup)` — exact name at implementation)
pairs interpolation-with-provenance with the typed parse: on pass-1 parse
failure, candidate leaves (provenance paths under the top-level `routes`
array whose value is i64/u64-clean — REST blocks, route templates, and
every other subtree keep today's semantics and are never probed) are
coerced to numbers in cloned trees, subsets tried smallest-first in
document order (k ≤ 8; 255 in-memory parses worst case), first parse
success wins, else pass 1's error. Every document that parses today is
unaffected (probing runs only after failure). `load_from_file_with_env`
and the discovery YAML arm route through the seam internally; the
camel-cli inline branch (runner.rs) switches its two calls
(`interpolate_yaml_source` + `parse_yaml`) to the seam — a mechanical
change plus a doc-comment refresh (its "numeric knobs stay on the rc-v1sw
track" note becomes false). REST-block extraction and route-template
bodies keep today's semantics (non-goals; template materialization parses
after doc-level parse and never reaches the probe).

**camel-config.** `resolve_tree_walk` stays string-only but records
structural paths (key/index segments — safe for keys containing `.` or
`[i]`; the existing dot-join renders diagnostics only) of token-carrying
leaves (provenance set) alongside resolution.
At the `merged_tree.try_into::<CamelConfig>()` boundary (config.rs:2942):
on failure, probe subsets of provenance leaves whose values parse as i64
(lexical `-?(0|[1-9][0-9]*)` guards leading zeros; then exact i64 parse
guards overflow) coerced to `toml::Value::Integer`, smallest-first, first
`try_into` success wins, else first error. Same probe cap as the DSL arm
(k ≤ 8) with the same first-pass-error fallback. Literal quoted numerics carry
no token, never enter the provenance set, stay rejected (pinned test
preserved). `CAMEL_CACHE_REPO_*` overrides merge before resolution:
token-bearing override values enter the provenance set and probe exactly
like file-authored leaves; token-free overrides keep today's typed
contract.

**camel-lint.** The validation copy applies the same observable carve-out,
derived from the embedded ROUTE_SCHEMA (which camel-lint already owns and
which covers every DSL field): a whole-scalar token with a clean integer
default (i64 or u64 parse; ROUTE_SCHEMA enforces the target type's bounds)
at an integer-typed schema position is validated as the NUMBER;
everywhere else as STRING `"d"` as today. Int position + clean default →
no diagnostic; int position + non-integer/overflow default → type Error
(boot rejects too); no-default tokens → Error; bool positions → Error
(out of scope). Corpus baselines re-recorded for affected fixtures.
Known divergence (documented, accepted): for pathological documents with
more than 8 candidate leaves boot returns the first error while lint
stays silent — same class as today's divergence, opposite direction.

### Clean integer, defined

Lexical form `-?(0|[1-9][0-9]*)` (ASCII digits, optional leading `-`, no
leading zeros, no whitespace, no plus), then a successful parse as i64
(camel-config) or i64-or-u64 (camel-dsl candidates; the real typed parse
enforces the exact field type afterwards). Leading-zero values are not
clean (YAML 1.1 octal ambiguity); `007` keeps string typing and fails as
today. Floats, bools, `1e3` are not clean (non-goals).

## Affected crates

- camel-dsl: provenance variant in env_interpolation + `parse_routes_with_env`
  seam + probe search + tests (demo, mixed doc, negative/overflow,
  discovery arm, mirror pins, fallback path pin).
- camel-config: provenance set in `resolve_tree_walk` + probe search at
  `try_into` + tests (coerce, literal-strict, overrides, mirror, overflow,
  leading-zero).
- camel-lint: typing-mirror carve-out from ROUTE_SCHEMA + baselines +
  tests + SYNC-mirror audit.
- camel-cli: runner.rs inline branch switches to the seam; stale typing
  doc-comment refreshed (lease: camel-cli.lock).

## Architecture boundaries

Parse-time tooling only; data/control plane untouched. The rc-93wct canon
keeps its guarantees (string-keeping seam, comments, escapes, fallback);
probing is a post-failure layer over the real parser, not a seam change.
camel-config keeps its separate resolver (CONTEXT-MAP Config entry; no
conflation with the DSL loader). Unit-tier LEAN semantics follow boot
through the shared seam — mock-testkit's boot-parity rationale now yields
acceptance, and its document `env:` map stays string-valued (ADR-0069
§13.1 untouched). No schema asset change: `xtask schema --check` stays
green untouched; the env-int-placeholder-parity rejection of `StringOrInt`
stands. Spec scenarios use only real DSL fields (`set_header` value,
`throttle.max_requests`, `circuit_breaker.open_duration_ms`,
`log_level`).

Spec home for the TOML arm: new capability `config-env-placeholders` (no
existing spec pins `resolve_tree_walk` strictness — the quoted-numeric
rejection is pinned by test only; `cache-repo-configuration` scopes both
its requirements to `cache_repo`).

## Phases

### Phase 1: DSL YAML arm
- **Goal:** integer placeholders load through every YAML seam; mixed docs
  keep string semantics.
- **Dependencies:** provenance variant; probe search; uniqueness argument
  pinned by unit test.
- **Externally-visible types/interfaces:** one new pub fn
  (`parse_routes_with_env` or equivalent); no signature changes.
- **Deliverable:** camel-dsl implementation + tests (demo shape, mixed
  doc, negative/overflow/leading-zero, discovery arm, mirror pins,
  fallback-path pin).
- **Exit-criteria:** demo parses env-unset (2) and env-set (5);
  `numeric_leaf_stays_string_after_interpolation` and set_header mirror
  tests green; JSON arm and fallback path unchanged.

### Phase 2: camel-config TOML arm
- **Goal:** quoted placeholder numerics coerce in integer-typed config
  fields; literals stay rejected.
- **Dependencies:** Phase 1 clean-integer rule.
- **Externally-visible types/interfaces:** none (`resolve_tree_with`
  signature unchanged; provenance internal).
- **Deliverable:** camel-config implementation + tests.
- **Exit-criteria:** `"${env:N:-8}"` coerces; literal `"1000"`,
  `CAMEL_CACHE_REPO_MAX_ENTRIES=notanumber`, leading-zero, and overflow
  values stay rejected.

### Phase 3: lint mirror, baselines, canon
- **Goal:** diagnostics match new boot behavior; docs truthful.
- **Dependencies:** Phase 1 landed.
- **Deliverable:** rschema carve-out + RON baselines + LSP session
  snapshot + runner.rs comment refresh + CONTEXT.md canon updates
  (camel-dsl, camel-config, camel-lint).
- **Exit-criteria:** int-position clean-int fixture yields no diagnostic;
  non-integer-default, no-default, bool fixtures unchanged.

## Alternatives considered

- **Hand-maintained integer table (revision 2):** rejected — proven
  incomplete (`timeout_ms`, `parallel_limit`, `threshold`,
  `max_entry_bytes`, …) and drift-prone; new numeric fields would fail
  open until manually added.
- **Dual-pass blanket re-emission (revision 1):** rejected — re-types
  string positions too, so mixed documents stay broken.
- **toml error-message key extraction:** rejected — the "for key" format
  is not pinned by any test; silent format drift would dead-loop the
  retry.
- **S2 per-field deserializers:** untagged `RouteDslStep` buffers through
  `deserialize_any`; type hints never reach leaf deserializers.
- **S3 scoped StringOrInt / schemars runtime walk:** public-contract and
  dependency-weight churn the parity design already rejected.
- **Bool/float positions, JSON route files, REST extraction, template
  bodies:** non-goals (follow-up bd); JSON never re-infers and failed
  pre-0.42 as well.
