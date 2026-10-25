# Design: 410-rschema-boolean-carveout

## Approach

Mirror the proven integer carve-out end-to-end (pre-flight e_gpt: approved
with additions, folded in below):

1. `struct BoolCandidate { path: String, span: Span, value: bool }` beside
   `IntCandidate` in `crates/camel-lint/src/rules/rschema.rs`. In
   `enforce_typing_mirror`'s `WholeScalarEnvToken::WithDefault` arm, next to
   the existing `clean_integer` check, add:
   `if let Some(value) = clean_boolean(&default) { bool_candidates.push(...) }`.
   The predicates are disjoint (`clean_integer("true")` is None;
   `clean_boolean("10")` is None), so check order is behaviorally neutral;
   keep the integer check first for minimal disturbance.
2. `fn clean_boolean(s: &str) -> Option<bool>` — exact match on `true` /
   `false`, nothing else — with a SYNC doc naming
   `camel-config::env_int_probe::clean_bool` and
   `camel-dsl::env_int_probe::clean_boolean`: "crate purity forbids the
   dependencies. Update all three together." The predicate consumes the same
   `default` text the `clean_integer` call site sees (the token's default
   slice, before `sanitize_env_value` string-forcing).
3. Generalize the carve-out consumer so ONE Condition A/B loop serves both
   candidate kinds (probe leaf replacement = `Value::Number` for integer
   candidates, `Value::Bool` for boolean candidates). Preserve the existing
   semantics exactly: Condition A = the STRING copy flags the leaf's error
   chain; Condition B = the probe clears that chain; carve (and return the
   leaf span for Info-note suppression) only on A∧B; any other outcome keeps
   STRING-copy behavior. Process integer candidates before boolean
   candidates. Spans are returned ONLY after a successful A∧B carve — never
   on mere candidate collection (pre-flight).
4. SYNC comment updates (comment-only, no behavior): `clean_bool` in
   `camel-config` and `clean_boolean` in `camel-dsl` gain "and camel-lint's
   rschema `clean_boolean`" in their SYNC headers. Module doc of rschema.rs
   (the `//!` block describing the integer carve-out) documents the boolean
   counterpart.
5. Tests in `crates/camel-lint/src/rules/rschema/tests.rs`:
   - `clean_boolean_lexical_gate` — SYNC gate pin in the
     `clean_integer_lexical_gate` style: accepts exactly `true` / `false`;
     rejects `""`, `TRUE`, `True`, `yes`, `on`, `1`, `0`, ` true`, `true `,
     `true\n`. This pins the local contract; cross-crate agreement is
     maintained by the reciprocal SYNC comments (the established convention —
     crate purity forbids a test dependency).
   - Row tests on `auto_startup` (the named real-world boolean field):
     `:-true` accepted, `:-false` accepted (zero Errors; substituted-default
     Info suppressed for the carved leaf, mirroring integer carve
     behavior); `:-yes`, `:-TRUE`, `:-1` each keep the type Error anchored
     on the placeholder; no-default `${env:AS}` keeps the unresolved Error;
     embedded `id: svc-${env:HOST:-true}` keeps non-whole-scalar semantics;
     boolean default at a STRING position (`set_header` value `:-true`)
     keeps the one-Info/zero-Error string semantics (no carve — Condition A
     fails); mixed document (clean integer default at `throttle.max_requests`
     + clean boolean default at `auto_startup`) produces no diagnostics.

## Affected crates

- `camel-lint`: boolean carve-out implementation + tests (the only
  behavioral change of this mission).
- `camel-config`: SYNC comment text only (`clean_bool` header).
- `camel-dsl`: SYNC comment text only (`clean_boolean` header).

## Architecture boundaries

Runtime (boot coercion), DSL semantics, and config-loader semantics are
untouched — 385 owns them and its behavior is the specification this mirror
copies. The change stays inside the assurance/lint plane (R-SCHEMA typing
mirror), preserving camel-lint's runtime-free, dependency-pure contract: no
new crate dependencies, no `std::env` reads, diagnostics remain a pure
function of document text. The predicate stays a private helper behind the
existing module layout; nothing is added to camel-lint's public API.

## Risk budget

In bounds: the files above; TDD (failing row tests first, then implement).
Out of bounds: `route-schema.json` regeneration (no fixture added), corpus
changes, runtime coercion, cross-crate dep introduction. Existing integer
tests (`bool_position_placeholder_still_error`,
`clean_integer_lexical_gate`) must stay green unmodified — they are the
regression fence for the shared carve-out loop.
