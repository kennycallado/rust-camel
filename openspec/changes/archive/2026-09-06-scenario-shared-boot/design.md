# Design: scenario-shared-boot

## Approach

Three-layer delegation so both commands literally share the composition
root wiring (ADR-0069 §4, §10):

1. **camel-dsl — env-injected discovery.** `discover_routes_inner` grows an
   optional `env_lookup: Option<&dyn Fn(&str) -> Option<String>>`; the
   interpolation site picks `interpolate_env_with(raw, lookup)` when
   injected, `interpolate_env(raw)` (process env) otherwise. New public
   entry `discover_routes_with_threshold_security_and_env(patterns,
   threshold, security_ctx, env_lookup)` gives scenario boot the full
   discovery contract — two-pass template materialization, size caps,
   reserved-test-suffix gate, JSON gating, source hashes — with hermetic
   `${env:}` resolution. Neither existing public entry changes behavior.
   Scenario boot passes the document's `routeFiles` (resolved against the
   project root) as literal patterns; literal paths are valid globs
   matching themselves. A pre-check that each file exists preserves the
   explicit missing-file error semantics (glob no-match is silent).

2. **camel-bundles — shared security/startup wiring.** New module
   `security_boot.rs` with two gating tiers:
   - Always available (uses only camel-bundles' existing hard dependencies
     — camel-core, camel-config; no new deps, no feature gate):
     `install_bind_exposure_acks(ctx, &CamelConfig)` — the run.rs:261-285
     block: ack map from `config.binds`, threaded to `ctx.
     set_bind_exposure_acks` and, under camel-bundles' existing `mcp` /
     `wasm` features, the MCP registry gate and the wasm source-bind gate
     (ADR-0061 Rule 4, fail-closed); `install_sql_startup_checks(ctx,
     &defs)` — `scan_route_definitions_for_sql_checks` →
     `ctx.add_startup_check` (ADR-0033); and
     `ensure_security_supported(&CamelConfig) -> Result<(), CamelError>`
     — the no-security validation path: a no-op when the `security`
     feature is enabled, and when disabled a fail-closed check that any
     `[security]` configuration yields a configuration error naming the
     required feature (the callable contract for the fail-closed scenario
     — callers in feature-less builds call it before boot).
   - Behind feature `security` (= `dep:camel-auth` +
     `dep:camel-component-keycloak` + `dep:camel-dsl`; the
     `SecurityCompileContext` type lives in camel-dsl):
     `build_security_compile_context_from_config(&CamelConfig, registry)`
     moved from `camel-cli/src/security.rs` (resolve_authenticators,
     register_providers, keycloak/UMA, with the same `wasm`/`not(wasm)`
     cfg variants keyed on camel-bundles' own features).
   camel-cli forwards `security` and lists it in its default feature set
   (camel-component-keycloak is a non-optional camel-cli dep today — the
   default build must keep the builder available or `camel run` behavior
   changes); its `run.rs` calls these helpers (same sequence, behavior
   unchanged — the extracted code is moved, not modified). When the
   feature is absent and a caller boot has `[security]` configured, the
   boot fails closed with a configuration error naming the required
   feature, mirroring the existing not-wasm variant's message shape.

3. **camel-integration-test — boot_scenario delegation.** Sequence mirrors
   run.rs:255-343 ordering exactly: sealed config load → context
   preparation → security compile-context build → bind-ack install →
   `camel_bundles::boot` cascade → route discovery → SQL startup checks
   from the discovered definitions → add routes → `ctx.start()`. Route
   loading goes through the env-injected discovery with
   `config.stream_caching.threshold` and the built security context;
   `load_route_definitions` (per-file `parse_yaml`) is deleted. Tier
   security gating (all checks run before any network or compilation):
   - camel-integration-test gains a `security` feature forwarding to
     `camel-bundles/security`. Without it, a config declaring `[security]`
     fails closed with a configuration error naming the feature.
   - `[security.keycloak]`-class configs (oidc; any network-prefetching
     provider) are rejected with `CamelError::AuthProviderUnavailable`
     (existing variant, camel-api error.rs:154): the tier runs offline,
     native provider only in v1.
   - `security.policies` / `security.permissions` (wasm authorization) are
     rejected fail-closed with a configuration error naming the v1 tier
     limitation (parity with the camel-cli not-wasm message shape); wasm
     security policies are a later wave.
   Error-class contract: the scenario-document runner classifies by error
   variant, never by message text (the convention document.rs states).
   The classification site for boot errors learns one rule — a
   `boot_scenario` error matching `CamelError::AuthProviderUnavailable`
   reports the `infra-unavailable` doc-error class; every other variant
   stays `full-boot-failure`.

## Affected crates

- camel-dsl: discovery refactor + new public entry + unit tests (injected
  lookup honored, process env not consulted, threshold/security threading,
  template materialization through the new entry).
- camel-bundles: `security_boot.rs` module (bind-acks and SQL-check
  helpers ungated; security builder behind `security`), optional deps
  camel-auth/camel-component-keycloak/camel-dsl; unit tests (native
  builder offline, ack install, SQL-check install, not-security
  fail-closed).
- camel-cli: `security.rs` reduced to a delegation shim (or deleted);
  `run.rs` calls the camel-bundles helpers; feature forwarding
  (`security = ["camel-bundles/security", ...]` added to the default
  feature set; `integration-http` gains
  `camel-integration-test/security`); scenario-tier tests in
  `commands/test/scenario.rs` (TempProject pattern, feature
  `integration-http`).
- camel-integration-test: `boot_scenario.rs` rewritten to delegate;
  `security` feature; unit tests for keycloak rejection + missing-file
  error + inline-route error semantics (preserved).

## Architecture boundaries

Runtime (camel-core/camel-bundles) owns the shared wiring; DSL (camel-dsl)
owns route discovery including the env-injection seam — no component or CLI
code enters camel-dsl. The CLI keeps `ctx.start()` ownership and the exec
gate (§10). camel-integration-test owns only the scenario-specific policy:
offline rejection of keycloak, LayeredEnv as the lookup source. Hermeticity
invariant: scenario boot reads env exclusively through `LayeredEnv::lookup`
injected into discovery and `from_file_sealed` — no `std::env::var` in the
tier path.

## Phases

### Phase 1: camel-dsl env-injected discovery
- **Goal:** the discovery seam that makes hermetic delegation possible.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `discover_routes_with_threshold_
  security_and_env`.
- **Deliverable:** camel-dsl change + unit tests; workspace builds.
- **Exit-criteria:** new-entry unit tests green (lookup honored, process
  env ignored, templates materialized, threshold/security threaded);
  existing discovery tests unchanged and green.

### Phase 2: camel-bundles shared wiring + camel-cli delegation
- **Goal:** one home for security ctx / bind acks / SQL checks.
- **Dependencies:** Phase 1 (not code-wise, but same change order).
- **Externally-visible types/interfaces:** `camel_bundles::{
  install_bind_exposure_acks, install_sql_startup_checks,
  ensure_security_supported}` ungated; `build_security_compile_context_
  from_config` behind feature `security`.
- **Deliverable:** camel-bundles module + camel-cli delegation + tests.
- **Exit-criteria:** camel run behavior unchanged (existing run tests
  green); camel-bundles unit tests green; no `camel-cli`-owned security
  builder code remains.

### Phase 3: scenario-boot delegation + tier tests
- **Goal:** `boot_scenario` boots through the shared root; every
  divergence row has a tier test.
- **Dependencies:** Phases 1-2.
- **Externally-visible types/interfaces:** none new (boot_scenario signature
  unchanged).
- **Deliverable:** boot_scenario rewrite + scenario tests + hermeticity
  regression.
- **Exit-criteria:** all seven acceptance tests green; existing scenario
  tests green; `cargo clippy`/`fmt`/xtask lints clean in touched crates.

## Alternatives considered

- **Builder in camel-config** (e_opus pick): defensible (config-adjacent)
  but ADR-0069 §10 names the shared boot — security setup, bind acks,
  startup checks — which is the camel-bundles seam; camel-config would
  also need the same new feature-gated deps. Rejected per e_glm decisive
  pick.
- **Hook-injection kept in camel-cli** (minimal diff): leaves the tier
  depending on CLI internals forever and does not converge the composition
  roots. Rejected.
- **Delegating to `CamelConfig::load_routes_with_security` as-is**:
  re-loads config with ambient env and reads `config.routes` patterns, not
  the document's `routeFiles` — a §4 violation. Rejected (e_glm caveat 1).
