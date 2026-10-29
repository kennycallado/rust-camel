# Proposal: jsonschema-format-enforcement

## Why

REST `request_schema` and the validator component compile JSON Schemas with
`jsonschema::validator_for`, which leaves format handling to the crate's
draft-dependent default: `format` is enforced for Draft 4/6/7 but is only an
annotation for 2019-09/2020-12, and a schema without `$schema` is treated as
2020-12. A REST operation declaring `format: email` therefore accepts
`not-an-email` today. Operators read `format: email` as a constraint; the
runtime silently ignores it on the most common draft. This is a correctness
gap (bd rc-giuvl), and fixing it is a behavior change: payloads that pass
today may be rejected after the fix.

## What Changes

- Both JSON-schema compile paths enable format validation explicitly with
  `jsonschema::options().should_validate_formats(true).build(..)`, which
  keeps `$schema` auto-detection (draft stays inferred) and forces format
  enforcement for every draft:
  - `crates/camel-processor/src/json_schema_validate.rs`
    (`JsonSchemaValidateService`, backs REST `request_schema`).
  - `crates/components/camel-validator/src/compiled.rs` (JSON and YAML
    schema compilation in the validator component).
- The validator component gains an opt-out:
  `[components.validator] validate-formats = false` (default `true`),
  implemented as a `ComponentBundle` following the `TemplateBundle`
  pattern; `camel-bundles` already depends on `camel-component-validator`,
  so no new dependency edge.
- REST `request_schema` stays always-on: there is no clean per-operation
  carrier for an opt-out, and no DSL key is invented without a spec.
- The two crates share no suitable common crate for a helper, so each gets
  a small local compile helper; neither adds a dependency edge.
- Docs: validator component page and REST DSL top-level-blocks page
  document format enforcement, the draft dependency, the opt-out, and a
  migration note.
- Excluded: pattern semantics, protobuf validation, remote format
  validators, other validators (MCP keeps its own behavior).

## Acceptance criteria

- `format: email` rejects a malformed value and accepts a valid one on both
  paths; `pattern` behavior is unchanged.
- Unknown formats stay annotations (crate policy `ignore_unknown_formats`
  default) and are stated in docs.
- A `$schema: draft-07` schema and a schema without `$schema` (2020-12)
  both enforce formats.
- `[components.validator] validate-formats = false` restores today's
  accept behavior in the validator component; invalid values fail config
  load naming the key.
- REST `request_schema` format enforcement has a route-level test.
- Gates: `cargo test -p camel-processor --test json_schema_validate`,
  `cargo test -p camel-validator`, REST route test, fmt, clippy
  (`-p camel-processor`, `-p camel-validator`, all targets,
  `-D warnings`), xtask lints, rustdoc for both crates, `mdbook test`,
  `openspec validate --all --strict`.

## Risk budget

Behavior change accepted and documented (changelog entry required).
Acceptable: strictness increase for malformed payloads that today pass.
Out of bounds: weakening the default without the explicit opt-out,
changing `pattern`, or a new DSL key for REST.

Bd: rc-giuvl
