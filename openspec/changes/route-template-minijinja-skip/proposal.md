# Proposal: route-template-minijinja-skip

## Why

The route-template placeholder engine substitutes every `{{name}}` in the
materialized template JSON. Inside a `language: minijinja` expression this
turns `{{ headers.name }}` into `TemplateError::UnknownParameter`, and a
declared template parameter named like a minijinja variable (e.g. `body`)
is silently substituted into minijinja sources. Route-template placeholders
and minijinja must not share syntax implicitly.

bd: rc-sanz1 (P2). Source: e_opus review of `camel-dsl/template/placeholder.rs`.

## What Changes

Affected crate: `camel-dsl` (template placeholder engine, materializer,
tests, docs). No API break; existing substitution semantics untouched.

- The materializer JSON walk detects expression objects carrying
  `language: "minijinja"` plus a string `source` (the only minijinja shape
  in the model; `simple`/`rhai`/`jsonpath`/`xpath` shorthands are distinct)
  and skips route-template placeholder substitution for that `source`.
  The minijinja text reaches the DSL compiler byte-identically.
- Declared route-template parameters never substitute inside a minijinja
  source, even on exact name collision.
- Outside minijinja nodes, declared-parameter substitution and
  unknown/missing-parameter validation stay unchanged: `{{unknown}}` still
  fails loudly.
- New escape hatch `\{{` renders a literal `{{` for minijinja-looking text
  in normal strings (in YAML plain/single-quoted scalars; JSON authors
  write `\\{{`). Existing `${...}` env semantics untouched.
- Docs: templated-routes section of `docs/src/yaml-dsl/top-level-blocks.md`
  explains the separation and the escape.
- Spec: delta under `openspec/changes/route-template-minijinja-skip/specs/`.

## Acceptance criteria

- `cargo test -p camel-dsl` green, including the four mission-mandated
  scenarios (minijinja source compiles with literal text; `body` collision
  not substituted; normal string and whole-node substitution unchanged;
  unknown placeholder still errors; escape hatch literal).
- `cargo fmt --check --all`; clippy `-p camel-dsl --all-targets -D warnings`;
  usual xtask lints; docs build for touched docs;
  `openspec validate --all --strict`.

## Non-goals

Changing minijinja language evaluation, redesigning the placeholder engine,
the external template component, filters/autoescape, or inline Simple
`${}` semantics.

## Risk budget

Acceptable: one walk-level skip keyed on a strict two-field shape, one
escape pre-pass, additive tests and docs.
Out of bounds: weakening validation for normal strings, minijinja runtime.
