# Proposal: 364-simpleset

## Why

Two load-time fallbacks silently change route behavior. An invalid `set_body` or
`transform` mapping becomes a JSON body after config parsing fails (rc-me8oe).
A declarative `script` with a non-mutating language becomes `set_body`, although
the Rust builder rejects it (rc-i7cfx). Both defects hide author errors.

The owner-delegated decision selects one mission with two independent breaking
implementation commits. Domain terms and boundaries follow `CONTEXT-MAP.md`:
DSL lowers route definitions; Runtime resolves registered Languages.

## What Changes

- Reject invalid value-source mappings during shared YAML/JSON lowering. Object
  literals require `value:`; scalars, arrays and null retain existing behavior.
- Narrow the JSON schema literal branch and regenerate both schema copies.
- Require `MutatingExpression` for `script` on all runtime entry points. Remove
  the declarative read-only fallback and share the compiler with the Rust builder.
- Migrate inventory sites, update operator documentation and preserve error
  propagation and redaction coverage.
- Affected crates: `camel-dsl`, `camel-core`, `camel-lint` (schema/tests),
  `camel-integration-test`, and `camel-test` (fixture migration).

Out of scope: aliases or key renames, AST deserializer redesign, TypeScript type
narrowing, new lint rules, static script capability checks, and unrelated bd bugs.
The opaque untagged diagnostic follow-up is rc-tyv0r.

### BREAKING

Wrap object literals in `value:` (`set_body: {value: {...}}`). Replace `script:`
that uses simple/jsonpath/xpath/minijinja with `transform: {language, source}`.
The behavior is identical.

For `transform` object literals, use the same `value:` wrapper. Custom read-only
languages also require migration from `script`. The two implementation commits
must each include `!` and a `BREAKING CHANGE:` migration footer, followed by its
bd trailer. Do not hand-edit CHANGELOG.md.

## Acceptance criteria

- YAML and JSON reject unknown-key, mixed-key and wrong-type mappings with
  `CamelError::RouteError`, valid alternatives and a wrapper hint, without values.
- Valid configs and wrapped objects retain behavior; schema rejects bare maps.
- Script capability errors are byte-identical across declarative and builder
  paths; canonical commands and reload compilation reject the same routes.
- Mutating scripts and existing runtime evaluation-error propagation still work.
- All breaking inventory sites are migrated, including four CXF fixtures and the
  additional README script example; surviving tests retain secret-redaction checks.
- A post-change parser sweep reports no failures caused by this change; prescribed
  quality gates, schema-check, doc-build and OpenSpec validation pass.

## Risk budget

Intentional risk: previously accepted routes fail at parse or compile time.
Unacceptable risks: value disclosure in diagnostics, changed successful route
semantics, or DSL dependence on a runtime language registry. Migration preserves
current literal objects, not presumed fixture intent. Infra-only verification is
CI-owned and must be reported explicitly. No main merge or issue closure here.
