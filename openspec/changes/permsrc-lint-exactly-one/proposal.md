# Proposal: permsrc-lint-exactly-one

## Why

Runtime route compilation requires a `security_policy.permission`
`resource`/`action` value source to specify exactly one of `literal`,
`header`, or `property` (`camel-dsl/src/yaml.rs::yaml_source_to_value_source`,
rc-gddb2 REJECT-ON-AMBIGUITY decision, retro520 territory 184 / 543530ef).
The generated `ROUTE_SCHEMA` that `camel lint` validates against permits
zero or multiple sources, so `camel lint` approves route files that route
compilation rejects — lint/runtime contract drift (bd rc-lkbqi, P2).

## What Changes

- Encode the exactly-one-non-null-source contract in the generated DSL
  schema: `RouteDslPermissionValueSource` (camel-dsl `route_ast.rs`) gains a
  schemars annotation so `route-schema.json` rejects zero-source and
  multi-source value specs. Explicit `null` siblings count as absent,
  mirroring serde `Option` deserialization exactly.
- `cargo xtask schema` regenerates `schemas/dsl/route-schema.json` and the
  byte-equal `crates/camel-lint/schema/route-schema.json` copy (gate
  `schema --check` enforces sync; TS artifacts regenerate unchanged).
- R-SCHEMA rule (`camel-lint/src/rules/rschema.rs`) renders the new oneOf
  failures as targeted diagnostics with field context, using the runtime
  error wording (`security_policy permission {field} must specify exactly
  one of: literal, header, or property (set: {found})`). All other oneOf
  failures keep today's collapsed diagnostic shape byte-identically.
- Corpus fixtures: negative (zero sources, multiple sources) with baseline
  entries, positive (exactly one) clean.
- Unit tests in `rschema/tests.rs` for both shapes, both fields
  (`resource`, `action`), null-sibling acceptance, and exactly-one.

Affected crates: camel-dsl (schema annotation only — zero runtime behavior
change), camel-lint (diagnostic rendering + tests), camel-cli (corpus
fixtures + baseline), scripts/xtask (no code change; regeneration only).

Explicitly excluded: TypeScript type shape (ts_rs derive unchanged — SDK
surface redesign is out of scope), runtime yaml.rs behavior (already
correct), other value-source resolvers.

## Acceptance criteria

- A route file with `permission.resource: {}` (or any zero-source form)
  produces an R-SCHEMA error naming the field and `(set: none set)`.
- A route file with two or more sources in one value spec produces an
  R-SCHEMA error naming the field and the found set (e.g.
  `(set: literal, header)`).
- A route file with exactly one source (or one source plus explicit null
  siblings) lints clean at R-SCHEMA.
- Existing oneOf diagnostics (CredentialSourceDsl, ExceptionDisposition,
  RouteDslRestBinding) are byte-identical before/after.
- `cargo xtask schema --check` passes; both schema copies byte-equal.
- Corpus zero-false-positives gate passes with the two new baseline entries.

## Risk budget

Acceptable: schema tightening newly rejects zero/multi-source authoring —
that is the intended contract enforcement (no in-tree examples/fixtures use
permission today, verified). Out of bounds: any runtime (camel-dsl yaml.rs)
behavior change, any change to non-permission R-SCHEMA diagnostic output,
TS type shape changes.
