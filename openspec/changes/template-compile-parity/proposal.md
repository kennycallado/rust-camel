# Proposal: template-compile-parity

## Why

Route templates (`route_templates:` + `templated_routes:`) compile through a
path that silently drops compile context. An empirical audit (bd rc-f2kp,
indexed report `rc-f2kp-template-audit`) verified against
`discover_routes_with_threshold_and_security`:

- A route with `security_policy: roles` compiles as a direct file but fails
  when materialized from a template — `materialize_and_compile` calls
  `compile_declarative_route`, which hardcodes `SecurityCompileContext::default()`
  (empty provider map), so `require_authenticator` rejects it and discovery
  aborts the whole file (F1, Critical).
- The configured `stream_cache_threshold` is likewise dropped; templates get
  the hardcoded default (F3, Important).
- One bad templated spec aborts the file/process and hides sibling diagnostics;
  template errors are flattened to "invalid template body" (F2 Critical, F11).
- Numeric/bool DSL fields cannot be parameterized: string-only substitution
  makes serde reject `"5000"` for `u64` (F4, Important).
- A single-route template instantiated N times with `route_id` override
  collides on the same id — discovery-fatal (F5, Important; blocks the
  6-instance secured-proxy dedup that motivated this work).
- `source_hash` keys on the template body, not resolved params, so hot-reload
  skips parameter changes (F6, Important).

## What Changes

Bring the template-materialization path to parity with the direct-route path,
in four delivery phases:

1. Thread `SecurityCompileContext` and `stream_cache_threshold` through
   `materialize_and_compile` (fixes F1, F3, F9).
2. Fail-closed but complete diagnostics: aggregate all templated-spec errors,
   preserve error class (fixes F2, F11).
3. Typed template parameters: declared type, whole-node substitution
   (fixes F4).
4. Per-instance route identity and parameter-sensitive `source_hash`
   (fixes F5, F6).

Deferred: templated `rest:`/`mcp:` blocks and OpenAPI visibility (F7, F8).

## Acceptance Criteria

- Secured templated routes compile when a real security context is configured;
  they still fail closed without one.
- Mixed files (direct + templated secured routes) materialize completely.
- Configured threshold reaches stream-cache steps of materialized routes.
- All failing templated specs are reported in one run; errors keep their class.
- Numeric parameters populate numeric DSL fields; non-coercible values are
  loud errors.
- N instances of a single-route template yield N distinct route ids.
- Changing a parameter value triggers hot-reload of that instance.

## Affected Crates

`camel-dsl` (discovery, template materializer, compile), `camel-api`
(`TemplateParameterSpec`), `camel-core` (reload tests), `camel-cli` (error
printing). Reference: bd rc-f2kp.

## Risk Budget

Compile-path change with fail-closed security semantics: no weakening of
existing validation is acceptable. Direct-route behavior must remain
byte-identical. Template contract changes (typed params, id rules) are
additive with string-typed defaults preserving current behavior.
