# Proposal: scenario-shared-boot

## Why

`crates/camel-integration-test/src/boot_scenario.rs` reimplements the
`camel run` composition root by hand and configures a strict subset of it,
violating ADR-0069 §4 ("both commands boot through the same composition
root", l.115-116). The e_opus/e_glm escalation passes (bd rc-6bsf) verified
the divergence inventory row by row:

1. Security compile context is never built — `parse_yaml` compiles with
   `SecurityCompileContext::default()`, so every `security_policy` route
   hard-fails to boot in the tier.
2. `[binds]` public-exposure acknowledgements (ADR-0061 Rule 4,
   fail-closed) are never installed — a non-loopback bind serving a `Public`
   route can never start in the tier; MCP/wasm bind gates lose their ack.
3. The configured `stream_caching.threshold` is not applied — routes
   silently compile with the workspace default.
4. Route templates are not materialized — a template route file yields zero
   routes (two-pass discovery is bypassed by per-file `parse_yaml`).
5. ADR-0033 fail-closed SQL startup checks are skipped — the tier
   greenlights routes production rejects at startup.

## What Changes

- **camel-dsl**: new env-lookup-injected discovery variant
  (`discover_routes_with_threshold_security_and_env`) refactored out of
  `discover_routes_inner`; existing entries keep process-env behavior
  unchanged. Closes the hermeticity trap: neither process env nor ambient
  env may leak into scenario boot (ADR-0069 §4).
- **camel-bundles**: new `security_boot` module in two tiers — the
  `[binds]` exposure-ack installer (context + MCP/wasm gates) and the
  ADR-0033 SQL startup-check installer are ungated (existing hard deps
  only); the security-context builder (extracted from
  `camel-cli/src/security.rs`) sits behind a `security` feature that adds
  the camel-auth, camel-component-keycloak, and camel-dsl dependencies.
  `camel run` delegates to these helpers (security forwarding in the CLI
  default feature set), behavior unchanged.
- **camel-integration-test**: `boot_scenario` stops per-file parsing — it
  resolves the document's `routeFiles` through the env-injected discovery
  (threading config threshold + security ctx), installs bind acks and SQL
  startup checks through the shared helpers, and rejects keycloak/oidc
  security configs with `CamelError::AuthProviderUnavailable` — the
  scenario runner classifies that variant as `infra-unavailable` (the
  tier is offline; v1 supports the native provider only).
- Tests: one test per divergence row at the scenario tier (TempProject
  pattern, `camel-cli/src/commands/test/scenario.rs`), plus a hermeticity
  regression test (`${env:}` present only in the process env is NOT
  resolved), plus camel-dsl/camel-bundles unit tests.

Excluded: the exec conditional gate (CLI-owned, route-content-conditional
per ADR-0069 §10), WASM `[beans]` loading, wasm security policies in the
tier, InMemoryJwks keycloak injection (future), and everything under epic
rc-enbw already filed (rc-ayke, rc-jjzy5, Wave A files: `camel-endpoint/
uri.rs`, `camel-http/lib.rs`, `adapters.rs`, `runner.rs`).

## Acceptance criteria

- A `security_policy` route with native security boots in the scenario tier.
- A non-loopback `Public` bind without ack fails closed in the scenario tier.
- The configured `stream_caching.threshold` reaches route compilation with
  the same wiring `camel run` uses.
- A templated route file materializes its routes at scenario boot.
- A `sql:` dynamic-query route fails closed at scenario startup (ADR-0033).
- `${env:X}` present only in the process env is not resolved by scenario boot.
- `camel run` behavior is unchanged (existing run tests green).

## Risk budget

Acceptable: new feature-gated deps (camel-auth, camel-component-keycloak)
behind camel-bundles `security`; refactoring discovery internals with
process-env behavior locked by existing tests. Out of bounds: any change to
`camel run` semantics, any ambient/process env read in scenario boot, any
network access in the tier, any edit to Wave A files.
