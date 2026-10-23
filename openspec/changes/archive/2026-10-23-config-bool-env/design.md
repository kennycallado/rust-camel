# Design: config-bool-env

## Approach

Mirror the proven integer-probe shape (env-int-placeholder-typing) with the
smallest possible delta in both arms. The probe stays post-failure: strict
deserialize first; only on failure does the candidate scan run.

**camel-config (TOML arm, `env_int_probe.rs`).** Add `fn clean_bool(s: &str)
-> Option<bool>` beside `clean_i64`: `Some(true)` for exactly `"true"`,
`Some(false)` for exactly `"false"`, `None` for everything else — no trim, no
case folding, no aliases. Candidate collection changes from
`Vec<(Vec<ConfigSeg>, i64)>` to `Vec<(Vec<ConfigSeg>, toml::Value)>`: a
provenance leaf resolving to a `String` yields `clean_i64(s).map(Value::Integer)`
or `clean_bool(s).map(Value::Boolean)`. Coercion writes the stored value. The
cap check, lexicographic smallest-first subset enumeration, first-pass oracle,
and final `Err(first_err)` are untouched; integer and boolean candidates share
one cap and one search (`MAX_PROBE_CANDIDATES = 8` counts both kinds).

**camel-dsl (YAML arm, `env_int_probe.rs`).** Add `fn clean_boolean(s: &str)
-> Option<serde_yml::Value>` beside `clean_integer`, returning
`Value::Bool`. `candidates()` accepts leaves whose text passes either gate;
`parse_with_probe` pairs each candidate path with its coerced value. The
`routes:`-only scope filter, cap, smallest-first document order, QUIET-parser
oracle, and the caller's single `error!` replay are unchanged. `try_coerced_subset`
already writes arbitrary `serde_yml::Value` leaves, so no subset machinery
changes.

**Correctness (minimal-subset property with mixed candidates).** Strictly
boolean-typed and strictly integer-typed positions reject strings, so every
parsing subset must coerce every typed position carrying a candidate; the
smallest-first search returns a minimal parsing subset. A `Boolean` coerced at
a string-typed or integer-typed position fails the strict parse (a strict
string field rejects a bool; serde expects i64), so wrong-kind coercion cannot
produce a smaller parsing subset than the correctly-typed one. Per the
pre-flight ruling we do NOT claim universal uniqueness ("exactly the strictly
typed positions") for branch-dependent custom deserializers — the spec wording
avoids that over-claim.

**Strictness boundary (recorded decision, pre-flight Q2).** Exactly `true` and
`false`. No case folding (`TRUE` rejected), no surrounding whitespace
(`"true "` rejected), no `1`/`0`, no `yes`/`no`. Rationale: a lenient boolean
parser is how `"yes"` becomes true in one provider and false in another; YAML
1.1's `yes/on` booleans are a live example in this codebase's own history
(the DSL spec's values-are-strings rule exists because of `enabled: on`).
Trimming was evaluated and rejected — if a strong user need appears, it is a
spec change, not a silent loosening.

**Error surface (recorded decision, pre-flight Q3).** When no subset
deserializes, the original first-pass error stands — it names the field (serde
path); the variable is named only by the existing unresolved-no-default error,
which fires before any probe. We do not introduce variable-plus-field error
text; that would change the shared error contract the integer probe pinned.

**Tests.** TOML arm in `crates/camel-config/tests/placeholder_e2e.rs`
(mirroring `literal_quoted_numeric_still_rejected_alongside_probe` and
`probe_cap_nine_config_candidates_fails`); YAML arm in the `env_int_probe.rs`
test module (the `probe_defaults` harness). Bool fields under test:
`CamelConfig.watch: bool` (config.rs) and route-level `auto_startup: bool`
(DeclarativeRoute, model.rs:243). String-mirror field: `log_level: String`.

## Affected crates

- camel-config: `src/env_int_probe.rs` (clean_bool + candidate/value generalization), `tests/placeholder_e2e.rs` (boolean scenarios).
- camel-dsl: `src/env_int_probe.rs` (clean_boolean + candidates pairing), test module.
- specs: delta under `config-env-placeholders` and `dsl`.
- docs: `docs/src/configuration/env-interpolation.md` (boolean rule, STE-rules apply).

## Architecture boundaries

Config/DSL boundary untouched: the camel-config arm keeps its own
`clean_bool` as a deliberate SYNC mirror of the DSL's `clean_boolean` (crate
purity forbids the dependency), matching the existing `clean_i64` /
`clean_integer` pair and its SYNC comments. camel-lint is NOT touched
(non-goals); the known lint/runtime divergence (`rschema.rs` integer carve-out
does not cover boolean coercion → false positive for `${env:X:-true}` at a
boolean route position) is filed as a follow-up bd at park time. No runtime,
component, or language-layer changes. Data/control plane boundary unaffected:
this is loader-layer typing only.

## Alternatives considered

- Lenient boolean parser (trim + case-fold + `1`/`0` + `yes`/`no`): rejected —
  cross-provider ambiguity, contradicts the clean-lexical-gate precedent.
- Separate boolean-only probe pass: rejected — two searches double the worst
  case and split the cap; one shared candidate universe keeps the
  smallest-first argument and the cap semantics identical to the integer probe.
- New error message naming variable and field on final failure: rejected —
  changes the pinned first-pass error contract (pre-flight Q3, option (a)).
- Touching camel-lint's rschema boolean carve-out in this mission: rejected —
  explicit non-goal; divergence is tracked as a follow-up bd instead.

## Spec-level shape

ADDED sibling requirements in both specs (not MODIFIED rewrites of the
integer requirements): the integer requirement texts remain the canonical
integer contract; the boolean requirement layers the second coercion onto the
same mechanism, stating the shared cap and oracle explicitly. This keeps the
delta reviewable and avoids restating two large requirements.
