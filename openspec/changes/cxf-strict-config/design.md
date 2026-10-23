# Design: cxf-strict-config

## Approach

One structural walk over the `toml::Table` before typed decoding, with a single
choke point on the type itself:

- `CxfPoolConfig` switches from `#[serde(try_from = "CxfPoolConfigRaw")]` to
  `#[serde(try_from = "toml::Table")]`. The new `TryFrom<toml::Table>` impl
  (1) runs the shape walk, (2) decodes `CxfPoolConfigRaw` from the table
  (existing assembly + `validate()` unchanged), so every deserialize path is
  strict, not just `CxfBundle::from_toml`.
- Why not `deny_unknown_fields` / `#[serde(flatten)]`: the mandated message
  must name the profile and the accepted location, which lives at a *different*
  level than the one that failed — serde's own error can do neither. Flatten
  would force Raw twins for `CxfProfileConfig`/`CxfSecurityFields` (breaking
  ~67 struct literals), drift hand-maintained field lists, and force redacting
  `Debug` impls on every new Raw (typo'd secret keys land in the buffer).
  This is the recorded deviation from the mission's "use serde's
  `deny_unknown_fields` where it fits" — it does not fit the message contract.
- Field lists come from the derived structs, not hand-written lists: a tiny
  recorder `Deserializer` captures the `FIELDS` slice the derive passes to
  `deserialize_struct` (works because none of these structs flatten). The
  recorder feeds an empty map; the resulting "missing field" error is ignored —
  `FIELDS` are captured before values are requested. No placeholder-value
  machinery, no drift.
- Walk: pool keys checked against `CxfPoolConfigRaw`'s fields; each
  `profiles[i]` against `CxfProfileConfig`; its `security` table against
  `CxfSecurityFields`; `security.tls` against `CxfTlsConfig`. A key absent at
  its level is classified: present in another cxf level's field list →
  "is accepted under <path>" (multi-location keys such as `keystore_path`,
  accepted at both `security` and `security.tls`, list every location);
  absent everywhere → "unknown key" plus the sorted accepted-keys list for
  that level.
- Profile label: profile-scoped diagnostics identify the entry as
  `profiles.<name>` from its `name` (string) when present, else
  `profiles[<index>]`; pool-level diagnostics identify `[components.cxf]`.
- Determinism and safety: all walker violations in one load are collected
  into a single `CamelError::Config`, sorted by location; diagnostics print
  key names, accepted locations, and (only as identity) the profile `name` —
  no other config values. Wrong-shape/type failures keep the typed decoder's
  existing error behavior.
- `listener_tls` (not in the raw TOML surface) gets a dedicated hint naming
  the five flat `soap_tls_*` keys.
- Sub-tables whose shape is wrong (`security = "x"`) are skipped by the walk;
  serde's own decode error reports them — no panic, no duplicate diagnostics.
- `reconnect` is not recursed into: `NetworkRetryPolicy` is owned by
  camel-component-api, outside this change's crate boundary.
- Env-var path: `CxfProfileEnvVars` is built from typed structs and only
  *emits* `CXF_PROFILE_*` variables to the Java bridge; nothing parses env
  vars back into config structs, so the strict TOML contract cannot break it,
  and unknown env vars are not config keys (out of scope by design).

## Affected crates

- `camel-cxf` (`src/config.rs`): walker + `TryFrom<toml::Table>`, message
  classification, unit tests.
- `camel-test` (`tests/cxf_test*`): positive regressions already cover valid
  configs; new fail-closed cases live in camel-cxf config tests.
- `docs` (`src/components/ws-soap.md`): accepted-keys-per-level table, contract
  statement, fail-closed note.

## Architecture boundaries

Components only: the strictness lives entirely inside `camel-cxf`'s config
surface; `camel-component-api` (`CamelError::Config`), the builder, DSL, and
runtime are untouched. The Java bridge is untouched (env-var contract is
outbound and unchanged). Docs update is user-facing contract documentation.

## Phases

Omitted — single-phase change.
