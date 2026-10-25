# Proposal: 410-rschema-boolean-carveout

## Why

Mission 385 (`config-bool-env`, bd rc-5mbm0 context) made whole-scalar
`${env:X:-true}` / `${env:X:-false}` defaults coerce into boolean route
positions at boot, in both arms: `camel-config`'s `clean_bool` (TOML) and
`camel-dsl`'s `clean_boolean` (YAML), exact lowercase `true`/`false`.
`camel-lint`'s R-SCHEMA typing mirror was explicitly out of scope for 385 and
still forces every substituted default to a JSON string, with only a
clean-INTEGER carve-out for number positions. Result: a document that now
loads cleanly (`auto_startup: ${env:X:-true}` under the strict-boolean
`auto_startup` schema field) is reported by R-SCHEMA as a type violation —
a false positive on a now-valid document, breaking the lint's boot-parity
contract.

## What Changes

- Add the boolean counterpart of the integer carve-out in
  `camel-lint`'s R-SCHEMA typing mirror: a whole-scalar
  `${env:X:-<default>}` token whose default the boot boolean rule accepts
  (exact lowercase `true`/`false`) gets a BOOLEAN validation copy at that
  leaf instead of the raw string instance — at a boolean-typed schema
  position, and only when the carve-out probe confirms the typed copy clears
  the leaf's violation (the same Condition A/B gate as the integer
  carve-out) — so no string-vs-boolean type violation is raised for that
  leaf. Structured as a candidates list consumed by `analyze` (mirroring
  `IntCandidate` / `integer_carve_out_instance`), not a special case inside
  the visitor.
- Add `camel-lint`'s boolean predicate as the third SYNC-annotated mirror of
  `camel-config::clean_bool` and `camel-dsl::clean_boolean` (existing
  cross-crate convention; crate purity forbids a dependency), pinned by a
  lexical-gate test in the `clean_integer_lexical_gate` style and reciprocal
  SYNC comment updates in all three crates.
- No change to runtime coercion (385 owns it), no schema regeneration, no
  new placeholder namespaces, no behavior change in `camel-config` /
  `camel-dsl` beyond SYNC comment text.

## Acceptance criteria

- R-SCHEMA accepts `auto_startup: ${env:X:-true}` and `:-false` with no
  Error (Info suppressed like carved integer leaves).
- `${env:X:-yes}` / `-TRUE` / `-1` at a boolean position still produce the
  type Error; no-default `${env:X}` at a boolean position unchanged;
  embedded (non-whole-scalar) placeholders unchanged.
- Integer default at a NUMBER position unchanged (existing carve-out rows).
- A drift-guard test pins the boolean predicate's accepted/rejected lexical
  set; SYNC comments updated in all three crates.
- `cargo test -p camel-lint` green; `cargo fmt --check`;
  `cargo clippy -p camel-lint --all-targets -- -D warnings`;
  `cargo xtask lint-single-source`; `cargo xtask lint-unwrap`.

## Risk budget

Acceptable: touching only `camel-lint` code + tests and comment-only SYNC
updates in `camel-config`/`camel-dsl`. Out of bounds: runtime coercion
changes, `route-schema.json` regeneration, new cross-crate dependencies,
shared-helper extractions with semantic delta.

Affected crates: `camel-lint` (code + tests), `camel-config`,
`camel-dsl` (SYNC comments only).

Bd: rc-5mbm0.
