# Proposal: jobtyped

## Why

The camel-job-interface epic (bd rc-d5cgc, e_opus RULING 2026-09-13 section
2 item 4) reserved typed job arguments as A4: the A2 `args:` block is
string-only, so an author cannot declare an integer, boolean, or closed-set
argument, and every `--arg` value reaches `${arg:NAME}` interpolation as a
raw string. Demand has surfaced (bd rc-g4ruv dispatched) — A4 is now in
scope as a pure extension of the landed A2 contract (archive
2026-09-14-jobargs) and the A3 help projection (archive 2026-09-14-jobhelp).

## What Changes

- Each argument declaration gains an OPTIONAL `type` key accepting `string`
  (default, A2 behavior unchanged), `int`, `bool`, and `enum[...]` (single
  string scalar with a comma-separated member list).
- At argument resolution, typed values (CLI pairs and defaults) are coerced
  to the declared type; a coercion failure is a validation error, exit 2,
  naming the argument, the expected type, and the raw value. Enum validates
  membership and lists the allowed members on failure.
- `${arg:NAME}` substitutes the coerced value's canonical string form
  (`007` → `7`, `TRUE` → `true`, enum member verbatim) across all
  interpolation sites (`to`, `body`, `headers`, `timeout`).
- A typed `default` that fails coercion is rejected at document load — the
  same declaration-check class `--help` already enforces.
- `camel job <NAME> --help` renders the DECLARED type in the type column
  (today hardcoded `string`): `int`, `bool`, `enum[a,b,c]`.
- Compiled artifacts coerce embedded typed defaults through the same seam
  at startup; the `--arg` surface stays closed on artifacts.
- The CONTEXT-MAP.md glossary entry `Declared job arguments (args:)` is
  updated to admit the optional `type` key with its grammar, canonical
  coercion, and the unchanged artifact `--arg` closure.

Excluded: A1 discovery and A3 help CONTRACTS (only the type-column
projection extends), the `${arg:}` scanner/stage in camel-dsl (BUSY —
zero touch), camel-http (BUSY — zero touch), new exit codes (taxonomy
frozen 2 > 1 > 0), version bumps (post-release 0.47.0 trunk).

## Acceptance criteria

- `type: int` rejects a non-integer `--arg` (exit 2, names the argument and
  expected type); strict i64 grammar, canonical decimal form substitutes.
- `type: bool` accepts `true`/`false` case-insensitively; anything else
  (including `1`/`0`) exits 2; canonical form is lowercase.
- `type: enum[a,b,c]` rejects a value outside the set (exit 2, lists the
  allowed members); membership is exact and case-sensitive.
- Omitted `type` → string: every landed A2 document behaves bit-identically.
- Coerced values substitute via `${arg:NAME}` in `to`, `body`, `headers`,
  and `timeout`, before field validation, through the existing
  interpolation stage.
- `--help` shows the declared type per argument; a typed default that
  fails coercion fails load (run AND `--help`) with exit 2.

## Risk budget

Acceptable: additive schema key under the existing deny-unknown-fields
per-argument map; new `JobDocError` variants inside the exit-2 class;
help-column width becomes dynamic. Out of bounds: any camel-dsl or
camel-http edit, any change to landed A1/A2/A3 scenario behavior for
untyped documents, any new exit code, any interpolation-stage change.

Bd: rc-g4ruv (parent epic rc-d5cgc).
