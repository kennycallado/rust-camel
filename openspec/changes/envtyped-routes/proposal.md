# Proposal: envtyped-routes

## Why

Route documents resolve `${env:...}` at the source-text level, so a substituted
value lands as a quoted string; every integer/boolean-typed position then
rejects it. camel-dsl already fixes this for leaves under the top-level
`routes:` key (loader-layer env-int probe, `env_int_probe.rs`), but the probe
scope is deliberately narrow:

- `rest:` blocks reject placeholders at typed positions
  (`RouteDslRest.port: u16`) — pinned today by
  `extract_rest_from_file_int_field_fails_type_mismatch` (rc-gykds was closed
  as a documented limitation).
- Route-template subtrees record the same symptom
  (`template/yaml.rs` regression-pin test).
- JSON route documents never interpolate through a provenance-bearing seam at
  all (discovery splices whole-text; no provenance, no probe).

The result is an inconsistent surface: the same document shape accepts
`${env:PORT:-8080}` under `routes:` and rejects it under `rest:`.

bd: `rc-3c67o.2` (parent epic `rc-3c67o`; sibling `.1` landed the camel-config
TOML arm as `637f6ff2d`).

## What Changes

- **Widen the probe scope** (camel-dsl `env_int_probe`): candidates become
  every provenance path the document parse can produce EXCEPT two stay-out
  subtrees — paths under the `templates` or `templated_routes` keys (template
  bodies are untyped at document parse; instance parameters are string-typed
  by contract) — so their numeric-looking leaves never consume cap budget.
  Subtrees whose typed positions cannot fail the document parse are
  documented and pinned (see design.md).
- **JSON provenance + probe seam** (camel-dsl): JSON tree-walk interpolation
  with provenance (`serde_json`-native, reusing the shared token/scalar
  helpers), and the single subset-search probe loop generalized over the tree
  representation (one loop, two glue impls: `serde_yml` and `serde_json`).
  Discovery's JSON arm and a new string-level seam use it.
- **`camel openapi generate` parity** (camel-dsl `extract_rest_blocks_from_file_with_env`):
  both arms run through the probe instead of hard-failing. The test pinning
  the old failure is REPLACED by a coercion test; the named-variable error for
  an unresolved no-default placeholder is kept.
- **Docs**: `docs/src/configuration/env-interpolation.md` typed-leaf section
  extended with the route-document surface and the openapi parity note.

Affected crates: `camel-dsl` (implementation + tests), `docs` (mdbook).
Explicitly excluded: `camel-config`, `camel-bundles`, the TOML arm (landed as
rc-3c67o.1), no new DSL keys, no `RouteDslRest` field-type changes, no new
dependencies, no `camel-config` dependency added to `camel-dsl` (crate purity
per the SYNC notes in both probe modules).

## Acceptance criteria

- `rest:` `port`/`host` from `${env:}` with default and with explicit lookup,
  in YAML and JSON, through the loader path (discovery) — not just the probe.
- A `templated_routes` numeric via the existing typed-parameter machinery
  loads end-to-end (fixture); template-body direct `${env:}` numerics keep
  today's documented limitation (outside the document-parse probe's reach).
- Embedded tokens (`p-${env:X}`), non-numeric values, leading zeros, i64/u64
  overflow, probe cap > 8: all keep the first-pass error.
- FIRST-PASS-ONLY promise: pass-1 behavior is unchanged — every document
  that parses today still parses with the same values (the probe runs only
  after a failed first pass). Probe-repair outcomes are not universally
  preserved: `routes:`-only probe repairs keep their outcome except in two
  pinned classes: widened candidates pushing the count over the cap
  (eight-throttle fixture + numeric-looking `$schema` root token keeps the
  first-pass error), and collision-guarded documents (a stale provenance
  path — a substituted entry overwritten by a later key — is dropped by the
  guard, so a doc that probe-loaded today ONLY through the stale path now
  keeps the first-pass error; a surviving literal must never coerce on a
  token's provenance). Widened repairs (`rest:`/`mcp:`) turn prior hard
  failures into loads — the purpose of the change.
- `camel openapi generate` accepts a `rest:` doc whose `port` is
  `${env:PORT:-8080}`; unresolved no-default placeholders still fail naming
  the variable.
- `gates-local` exit 0, workspace build/clippy green, mdbook test green for
  the changed doc page.

## Risk budget

- Acceptable: probe attempt count grows slightly for documents mixing `rest:`
  and `routes:` candidates (cap unchanged: 8 combined candidates).
- Not acceptable: any document that parses today changing behavior; relaxed
  literal typing (quoted numerics without tokens stay rejected); probe
  heuristics replacing the real parser as the type oracle.
