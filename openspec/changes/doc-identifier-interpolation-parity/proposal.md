# Proposal: doc-identifier-interpolation-parity

## Why

bd rc-4hexo (P2 regression in released v0.42.0, reported by the camel-cache
demo team): `camel test` name-matches route-side identifiers against raw
doc-side keys. Since ac147d07 the route side interpolates
`${env:NAME:-default}` default-only (`camel_dsl::load_from_file` =
`load_from_file_with_env(path, &|_| None)` via `interpolate_yaml_source`),
while the doc side (`TestDocument`) is serde-parsed RAW. v0.41.0 matched by
coincidence (neither side interpolated); v0.42.0 breaks with
"repository 'persistent' is not registered" whenever a doc declares
`repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`
next to a route referencing `${env:CACHE_REPO_NAME:-persistent}`. The
regression is env-independent (fails with the variable set or unset).

## What Changes

- `parse_test_document` gains a step (a0), immediately after deserialization
  and before ALL validation: identifier fields are interpolated with the
  camel-dsl env scanner (`interpolate_env_with`, default-only lookup
  `&|_| None`) — same grammar and error shape as route sources.
- Five identifier field-groups interpolate: `repositories` keys (all three
  registry maps), `beans` keys, `intercepts` source URIs AND action targets,
  `mock:` references in `expects` keys and `sequence` entries, `inputs[].to`.
- Assertion data stays literal (hermeticity): input bodies/headers,
  `expectReply`, expectation matcher contents, bean `methods`/`config`
  values, repository stub targets, `settle`, and the route source fields.
- New `TestDocError::EnvUnresolved { var, field }`: a no-default
  `${env:X}` in an identifier field fails doc-validation (exit 2) naming
  the VARIABLE and the field position, wording mirroring the route side.
- Escape parity for free via the engine: `$${env:...}` stays literal.
- Post-interpolation key collisions in one map are document errors.
- Spec: ADD requirement "Doc-side identifiers interpolate default-only for
  name-match parity with route sources" to `mock-testkit`.
- Explicit non-goal: path/glob fields (`routeFiles`, `routeFilesFromRoot`)
  never interpolate.

## Impact

- Code: `crates/camel-cli/src/commands/test/document.rs` only (helper,
  error variant, step (a0) map rebuilds). No engine change: camel-dsl,
  camel-lint, camel-lsp, and the runner register logic are untouched. No
  new dependencies (camel-cli already depends on camel-dsl).
- Tests: parse-level unit tests for every field-group + failure modes;
  integration test flipping the rc-4hexo repro FAIL→PASS; anti-widening
  witness; escape pin; routeFiles non-goal pin.
- Docs: `crates/camel-cli/CONTEXT.md` hermeticity note extended with the
  identifier-interpolation carve-out.
- Design coupling (rc-l7m7t): the `&|_| None` closure is the future
  injection point for the LayeredEnv doc-env layer on BOTH sides; nothing
  here may hardcode against that swap.
