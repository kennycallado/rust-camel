# Proposal: env-int-placeholder-typing

## Why

Since 0.42.0, `${env:}` placeholders in int-typed DSL fields fail to load.
`max_requests: ${env:DWD_WARM_MAX_REQUESTS:-2}` in a throttle step yields a
string-typed leaf `"2"`, and `ThrottleStepDef.max_requests: usize` rejects it
("data did not match any variant of untagged enum RouteDslStep"). The rc-93wct
tree-walk keeps substituted leaves string-typed on purpose (mirror-case
safety), and its whole-text fallback fires only on YAML round-trip failure —
never on typed-parse failure. Pre-0.42 whole-text splice re-inferred ints, so
this is a regression. The camel-cache demo team is hard-blocked on 0.41.0 and
cannot upgrade (bd rc-45xig).

The same gap exists in camel-config TOML: `resolve_tree_walk` visits string
leaves only, and typed TOML syntax blocks bare placeholders in int slots, so
no env-driven numeric knob is possible (bd rc-v1sw).

## What Changes

- **camel-dsl** — provenance-tracked interpolation plus a probe search at
  the typed boundary. The tree walk additionally records which leaves were
  whole-scalar `${env:...}` tokens; when the typed parse fails, the loader
  tries coerced copies of the interpolated tree — candidate leaves
  (recorded leaves whose value parses as an integer) become numbers,
  subsets smallest-first, and the real typed parse is the oracle. The
  first subset that parses wins; if none does, today's error is returned.
  Every document that loads today is untouched (probing runs only after
  failure), and mixed documents (string field receiving `"123"` and
  integer field receiving a placeholder together) load correctly — the
  successful subset contains exactly the integer positions. One new
  public seam for callers that pair interpolation with parsing; existing
  signatures unchanged. The probe is bounded (more than eight integer
  candidates keeps today's error) and collects candidates only under the
  `routes` array.
- **camel-config** — the same probe search at the config deserialize
  boundary. The resolver records which leaves carried a `${env:` token;
  on deserialize failure, subsets of those leaves whose values parse as
  i64 coerce to TOML integers and deserialization retries, smallest-first,
  with the same eight-candidate cap and first-error fallback. Literal
  quoted numerics (no placeholder token) stay rejected.
- **camel-lint** — typing mirror update: integer-position whole-scalar
  tokens with a clean-integer default validate as the number (derived
  from the embedded route schema) and emit no diagnostic; non-integer
  defaults, no-default tokens, and bool positions keep the Error.

Non-goals: bool and float positions (follow-up bd), JSON route files (JSON
never re-infers; failed pre-0.42 too), `StringOrInt` public-contract changes
(rejected in the env-int-placeholder-parity design), any schema asset change.

Mirror-case safety is structural: a successful coerced subset cannot
include a string-position leaf (an integer there fails the parse), so a
string field receiving `"123"` is unreachable by the fix.

## Acceptance criteria

- Throttle step `max_requests: ${env:DWD_WARM_MAX_REQUESTS:-2}` parses and
  compiles through the real loader (`load_from_file_with_env`) and the
  discovery arm, with the env unset (default `2`) and set to `5`.
- String positions with substituted numeric-looking values keep byte-identical
  string semantics (mirror case; existing seam tests stay green).
- camel-config TOML: `field = "${env:N:-8}"` in an int-typed position resolves
  and coerces; literal `timeout_ms = "1000"` stays rejected.
- Unresolved placeholders keep today's caller-facing wording in both arms
  (the DSL seam types them as `RoutesEnvError::Unresolved(var)`; the TOML
  resolver's existing error shape is untouched).
- `cargo xtask schema --check` passes unchanged (no schema asset touched).

## Risk budget

Acceptable: lint corpus baseline churn for flipped int-position
diagnostics; narrow spec MODIFIED sections (dsl, route-lint, mock-testkit)
plus one ADDED capability (config-env-placeholders); a bounded probe
(documents with more than eight integer candidates keep today's error).
Out of bounds: changing outcomes for currently-loading documents, any
public DSL type signature, the embedded route schema, or the strict-literal
TOML rejection.
