# Tasks: scenario-shared-boot

## Phase 1: camel-dsl env-injected discovery

### camel-dsl

#### Task 1.1: env-lookup-injected discovery entry

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified)

**Steps:**
1. Change `discover_routes_inner` signature to
   `fn discover_routes_inner(patterns: &[String], stream_cache_threshold: Option<usize>, security_ctx: Option<SecurityCompileContext>, env_lookup: Option<&dyn Fn(&str) -> Option<String>>)`; at the
   interpolation site (currently `interpolate_env(&raw_content)` around
   discovery.rs:310) pick `interpolate_env_with(&raw_content, lookup)` when
   `env_lookup` is `Some`, `interpolate_env(&raw_content)` otherwise; map to
   the existing `DiscoveryError::Env` in both arms.
2. Update the three existing public entries
   (`discover_routes`, `discover_routes_with_threshold`,
   `discover_routes_with_threshold_and_security`) to pass `None` for
   `env_lookup` — no behavior change.
3. Add public entry:
   ```rust
   pub fn discover_routes_with_threshold_security_and_env(
       patterns: &[String],
       stream_cache_threshold: usize,
       security_ctx: SecurityCompileContext,
       env_lookup: &dyn Fn(&str) -> Option<String>,
   ) -> Result<Vec<RouteDefinition>, DiscoveryError>
   ```
   delegating to `discover_routes_inner` with `Some` for threshold,
   security ctx, and lookup. Doc-comment: hermetic callers (the
   integration tier) inject their layered environment; process env is
   never consulted through this entry.
4. Export the new entry from `crates/camel-dsl/src/lib.rs` alongside the
   existing discovery re-exports.

**Tests:** (in `discovery.rs` test module; use `tempfile` dev-dep already
present; each test writes one route file in a temp dir and passes its
absolute path string as the literal pattern)
- `env_injected_entry_resolves_through_lookup`: route file with
  `from: direct:${env:RC_TIER_ONLY}` → call
  `discover_routes_with_threshold_security_and_env(&[path], 4096,
  SecurityCompileContext::default(), &|n| (n == "RC_TIER_ONLY").then(|| "start".into()))`
  → Ok; returned route's from-URI is `direct:start`.
- `env_injected_entry_never_reads_process_env`: route file with
  `${env:RC_6BSF_PROC_ONLY}`; test first sets
  `std::env::set_var("RC_6BSF_PROC_ONLY", "leak")` (unique name; remove
  with `std::env::remove_var` at end), lookup returns `None` for
  everything → Err matching `DiscoveryError::Env { var_name, .. }` with
  `var_name == "RC_6BSF_PROC_ONLY"` (proves the process value "leak" was
  never applied).
- `env_injected_entry_materializes_templates`: route file declaring one
  template (one `direct:${env:RC_TPL}` endpoint parameter) and two
  templated routes referencing it → call the new entry with a lookup
  resolving `RC_TPL` → Ok with 2 routes; compare against
  `discover_routes_with_threshold_and_security(&[path], 4096,
  SecurityCompileContext::default())` run on a second file whose
  placeholders are pre-substituted to the same values using the existing
  discovery-test comparison convention (`RouteDefinition` has no
  `PartialEq`): assert equal `len()`, equal `route_id()` sets in order,
  and equal from-URI strings per route (threading parity: same
  threshold, same security ctx, same materialization).
- `env_injected_entry_equivalent_output_at_same_threshold`: bare
  `stream_cache: {}` step route file → new entry with threshold 777 vs
  `camel_dsl::yaml::parse_yaml_with_threshold(&pre_interpolated_content,
  777)` over the same content after manual substitution → both return the
  same single route (proves the threshold parameter reaches parsing
  through the new entry).

**Acceptance:**
- `cargo test -p camel-dsl discovery` passes including the four new tests.
- `cargo test -p camel-dsl` passes with zero modifications to existing
  tests (behavior-unchanged gate).
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` exit 0.

- [x] 1.1

## Phase 2: camel-bundles shared wiring + camel-cli delegation

### camel-bundles

#### Task 2.1: ungated shared installers (bind acks, SQL checks, security gate)

**Files:**
- `crates/camel-bundles/src/security_boot.rs` (new)
- `crates/camel-bundles/src/lib.rs` (modified — add `pub mod security_boot;`)

**Steps:**
1. In `security_boot.rs` add
   `pub async fn install_bind_exposure_acks(ctx: &mut CamelContext, config: &CamelConfig)`: build
   `bind_acks: HashMap<String, bool>` from `config.binds` exactly as
   `crates/camel-cli/src/commands/run.rs:263-266` does; under
   `#[cfg(feature = "mcp")]` call
   `camel_component_mcp::McpServerRegistry::global().set_bind_exposure_acks(bind_acks.clone())`;
   under `#[cfg(feature = "wasm")]` call
   `camel_component_wasm::WasmSourceBindAcks::global().set(bind_acks.clone())`
   (the body of run.rs `install_wasm_bind_acks`, inlined); finally
   `ctx.set_bind_exposure_acks(camel_core::route_controller::BindExposureAcks::new(bind_acks)).await`.
2. Add `pub fn install_sql_startup_checks(ctx: &mut CamelContext, defs: &[RouteDefinition])`:
   `for check in camel_core::startup_validation::scan_route_definitions_for_sql_checks(defs) { ctx.add_startup_check(check); }`
   (the run.rs:339-343 block).
3. Add `pub fn ensure_security_supported(config: &CamelConfig) -> Result<(), CamelError>`:
   `#[cfg(feature = "security")]` variant returns `Ok(())`; `#[cfg(not(feature = "security"))]`
   variant returns `Err(CamelError::Config(...))` when ANY security
   section is configured — explicit field check per `SecurityConfig`
   (camel-config `config.rs:1376-1387`):
   `config.security.oidc.is_some() || config.security.native.is_some() || config.security.keycloak.is_some() || config.security.permissions.is_some() || config.security.policies.is_some()`
   (the struct has no `is_empty`) — the error names the
   `camel-bundles/security` feature; `Ok(())` otherwise.
4. Unit tests in the same file.

**Tests:**
- `install_bind_exposure_acks_installs_from_config`
  (`#[cfg(feature = "wasm")]` — the assertion probe needs the wasm
  feature, which is in camel-bundles defaults): build a `CamelConfig`
  via `toml::from_str` with `[binds."127.0.0.1:41999"] allow_public_exposure = true`
  (mirror of run_tests.rs `wasm_bind_acks_wired_from_config`) → call
  `install_bind_exposure_acks(&mut ctx, &config)` on a fresh
  `CamelContext` (match how camel-bundles tests construct one today; if
  no precedent, use `camel_config::CamelConfig::configure_context` on an
  empty config) → assert
  `camel_component_wasm::WasmSourceBindAcks::global().acknowledged("127.0.0.1:41999")`.
- `install_sql_startup_checks_registers_checks`: build one
  `RouteDefinition` whose steps reference a `sql:` endpoint shaped like
  the dynamic-query fixture in
  `crates/camel-core/src/startup_validation.rs` tests (~lines 402-437 —
  the shape that yields a check) → call
  `install_sql_startup_checks(&mut ctx, &[def])` in a `#[tokio::test]` →
  `ctx.start()` fails with a startup-validation error whose message
  names `sql-dynamic-query` (the ConfigCheck id,
  startup_validation.rs:81).
- `ensure_security_supported_without_feature_rejects_security`
  (`#[cfg(not(feature = "security"))]`): a `CamelConfig` with
  `[security.native]` set → `ensure_security_supported` returns
  `Err(CamelError::Config(msg))` where `msg` contains
  `camel-bundles/security`; an empty security config returns `Ok(())`
  (the `Ok(())` half also holds under the security feature — add a
  cfg-invariant companion asserting it under
  `#[cfg(feature = "security")]` in task 2.2's test file).

**Acceptance:**
- `cargo test -p camel-bundles` passes with the new tests.
- `cargo check -p camel-bundles --no-default-features` exits 0 (ungated
  helpers compile without the security feature).
- `cargo fmt --check` and `cargo clippy -p camel-bundles -- -D warnings` exit 0.

- [x] 2.1

#### Task 2.2: security-context builder behind feature `security`

**Files:**
- `crates/camel-bundles/src/security_boot.rs` (modified)
- `crates/camel-bundles/Cargo.toml` (modified)

**Steps:**
1. In `Cargo.toml` add optional deps `camel-auth`, `camel-component-keycloak`,
   `camel-dsl` (all `{ workspace = true, optional = true }`) and feature
   `security = ["dep:camel-auth", "dep:camel-component-keycloak", "dep:camel-dsl"]`.
2. Move the builder from `crates/camel-cli/src/security.rs` into
   `security_boot.rs` behind `#[cfg(feature = "security")]`: the
   `resolve_authenticators`, `register_providers`,
   `register_keycloak_uma_evaluator`, native/keycloak authenticator
   helpers, and both cfg variants of
   `pub async fn build_security_compile_context_from_config(camel_config: &CamelConfig, registry: Arc<std::sync::Mutex<camel_core::Registry>>) -> Result<SecurityCompileContext, CamelError>`
   — verbatim except crate-path fixes (`camel_dsl::SecurityCompileContext`
   import), the wasm variants keyed on camel-bundles' own `wasm`
   feature instead of camel-cli's, and the two not-wasm rejection
   messages rewritten from "requires camel-cli wasm feature" to
   "requires the camel-bundles wasm feature" (the crate owner changed).
3. Move the security.rs unit tests that cover the builder into
   `security_boot.rs` under `#[cfg(all(test, feature = "security"))]`.
4. Add `#[cfg(feature = "security")] use` for `SecurityCompileContext` and
   re-export `build_security_compile_context_from_config` from `lib.rs`
   doc-visible surface (plain `pub` in the module; no glob re-export).

**Tests:**
- `native_security_builder_offline`: `CamelConfig` with a native
  credential (`[security.native]` with an inline secret — copy the config
  shape from the moved security.rs tests) → builder returns Ok and the
  context compiles a `security_policy` route (copy the smallest
  security_policy route fixture from the moved tests) — no network
  touched (StaticTokenAuthenticator path).
- `builder_rejects_wasm_security_without_wasm_feature`
  (`#[cfg(not(feature = "wasm"))]` — runs under
  `--no-default-features --features security`): a config with
  `[security.policies]` set → builder returns
  `Err(CamelError::Config(msg))` with msg containing
  `requires the camel-bundles wasm feature`.
- `ensure_security_supported_ok_with_feature`
  (`#[cfg(feature = "security")]`): empty security config →
  `ensure_security_supported` returns `Ok(())` (companion to 2.1's
  not-security test; cfg-invariant pair).

**Acceptance:**
- `cargo test -p camel-bundles --features security` passes including the
  moved + new tests (wasm variant; not-wasm test compiles out).
- `cargo test -p camel-bundles --no-default-features --features security`
  passes (not-wasm builder variant; the rejection test runs here).
- `cargo test -p camel-bundles` (defaults, no security) still passes
  (2.1 tests).
- `cargo check -p camel-bundles --no-default-features --features security` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-bundles --features security -- -D warnings` exit 0.

- [x] 2.2

### camel-cli

#### Task 2.3: camel run delegates to the shared helpers

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-cli/src/security.rs` (modified — reduced or deleted)
- `crates/camel-cli/src/commands/run_tests.rs` (modified)
- `crates/camel-cli/src/lib.rs` (modified — drop `mod security;` when the
  file is deleted)
- `crates/camel-cli/Cargo.toml` (modified)

**Steps:**
1. In `Cargo.toml`: add feature
   `security = ["camel-bundles/security"]` and include `"security"` in the
   `default` feature list (default builds must keep the builder available
   or `camel run` behavior changes).
2. In `run.rs`: replace the inline bind-acks block (lines ~261-285) with
   `camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, &camel_config).await;`
   ; delete the now-unused local `install_wasm_bind_acks` helper
   (~line 551-560); replace the security build call
   `crate::security::build_security_compile_context_from_config(...)` with
   `camel_bundles::security_boot::build_security_compile_context_from_config(...)`
   under `#[cfg(feature = "security")]`, plus a preceding
   `camel_bundles::security_boot::ensure_security_supported(&camel_config)?;`
   under `#[cfg(not(feature = "security"))]`; replace the SQL-check block
   (~lines 339-343) with
   `camel_bundles::security_boot::install_sql_startup_checks(&mut ctx, &defs);`.
   Sequence after the edit must read: security build → bind-ack install →
   `camel_bundles::boot` → discovery → exec gate → SQL checks → add
   routes → start (unchanged from today's run.rs order).
3. Reduce `crates/camel-cli/src/security.rs` to nothing (delete the file
   and its `mod security;` declaration) if nothing else in camel-cli
   references it — grep `crate::security` and `mod security` first; if
   other references exist, reduce the file to only what those references
   need and leave a comment pointing at camel-bundles.
4. In `run_tests.rs`: update `wasm_bind_acks_wired_from_config` to call
   `camel_bundles::security_boot::install_bind_exposure_acks` instead of
   the deleted local helper (same construction, same assertion); update
   any test importing the old security builder path.

**Tests:**
- `wasm_bind_acks_wired_from_config` (updated): same arrange/assert as
  today, action now calls the shared installer → assertion unchanged
  (`WasmSourceBindAcks::global().acknowledged(TEST_BIND)`).
- `run_shared_wiring_native_credentials_gate` (new, `#[tokio::test]`,
  proves the camel-run half of the runtime-boot "both callers boot a
  security_policy route" scenario): temp dir with Camel.toml (native
  bearer-token credential — the fixture shape from the security tests
  moved into `crates/camel-bundles/src/security_boot.rs` by task 2.2)
  and a `routes.yaml` with `from: direct:sec → security_policy →
  to: mock:out`; ACT: replicate the run.rs wiring sequence in-process —
  `CamelConfig::configure_context_with_beans(&config, None)` →
  `camel_bundles::security_boot::build_security_compile_context_from_config(&config, ctx.registry_arc())` →
  `install_bind_exposure_acks` → `camel_bundles::boot` →
  `camel_dsl::discover_routes_with_threshold_and_security(&["routes.yaml".into()],
  config.stream_caching.threshold, sec)` → `install_sql_startup_checks`
  → add routes → `ctx.start()`; ASSERT: a direct send to `direct:sec`
  WITH the valid bearer token (drive stimulus exactly as
  `DirectStimulus` does — `ctx.producer_context()`, create the direct
  producer through the component registry, `producer.oneshot(exchange)`;
  read-only reference at `crates/camel-integration-test/src/adapters.rs`
  ~646-680) results in the message arriving at `mock:out` (mock
  component message store); a second send WITHOUT credentials fails
  with `CamelError::Unauthenticated` (camel-auth maps every native
  refusal shape to that variant, types.rs:36-38).
- `run_shared_wiring_public_bind_without_ack_fails` (new, `#[tokio::test]`,
  proves the camel-run half of the runtime-boot "both callers refuse an
  unacknowledged public bind" scenario): same wiring sequence with
  `[binds."0.0.0.0:41997"]` (no `allow_public_exposure`) and a route
  `from: http:0.0.0.0:41997/pub` with Public exposure → ASSERT:
  `ctx.start()` fails with `CamelError::RouteError` whose message
  contains `non-loopback address; acknowledge via [binds` (the
  `enforce_bind_exposure_gate` refusal, camel-auth bind_gate.rs:74-79).
- Existing run/security tests pass unmodified otherwise (the moved
  security.rs tests now live in camel-bundles; if a camel-cli test
  referenced them by path, update the import only).

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (camel run behavior unchanged —
  this is the runtime-boot "delegates without behavior change" gate; the
  `wasm_bind_acks_wired_from_config` update is a mechanical call-site
  swap with assertions unchanged).
- `cargo test -p camel-cli --lib run_shared_wiring` passes (the two
  both-callers run-side tests).
- `rg -n 'default = \[.*"security"' crates/camel-cli/Cargo.toml` returns
  at least one line (machine check: security is in the CLI default
  feature set — without it, default `camel run` builds would silently
  lose the security builder).
- `grep -rn "build_security_compile_context_from_config" crates/camel-cli/src/`
  returns no camel-cli-local definitions (only the camel-bundles call).
- `cargo check -p camel-cli --no-default-features` exits 0.
- `cargo fmt --check` and
  `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 2.3

## Phase 3: scenario-boot delegation + tier tests

### camel-integration-test

#### Task 3.1: boot_scenario delegates to the shared composition root

**Files:**
- `crates/camel-integration-test/src/boot_scenario.rs` (modified)
- `crates/camel-integration-test/src/lib.rs` (modified — wire the
  `boot_scenario_test` module next to the existing `*_test.rs` modules)
- `crates/camel-integration-test/Cargo.toml` (modified)
- `crates/camel-integration-test/src/boot_scenario_test.rs` (new)

**Steps:**
1. In `Cargo.toml` add feature `security = ["camel-bundles/security"]`
   (camel-bundles is already a dependency).
2. Rewrite `boot_scenario` to the run.rs:255-343 sequence:
   (a) sealed config load (unchanged `from_file_sealed`);
   (b) `CamelConfig::configure_context_with_beans(&config, None)` (unchanged);
   (c) tier security gating BEFORE any builder call:
   `config.security.keycloak.is_some() || config.security.oidc.is_some()`
   (the `SecurityConfig` fields at camel-config config.rs ~1378; include
   every network-prefetching provider field present) → return
   `Err(CamelError::AuthProviderUnavailable("scenario tier runs offline (no network): keycloak/oidc security requires a network-prefetching auth provider; v1 supports the native provider only"))`;
   `config.security.policies.is_some() || config.security.permissions.is_some()`
   → return `Err(CamelError::Config("wasm security policies/permissions are not supported in the scenario tier in v1 (offline tier; later wave)"))`;
   (d) security compile context:
   `#[cfg(feature = "security")]` →
   `camel_bundles::security_boot::build_security_compile_context_from_config(&config, ctx.registry_arc()).await?`;
   `#[cfg(not(feature = "security"))]` →
   `camel_bundles::security_boot::ensure_security_supported(&config)?` and
   use `SecurityCompileContext::default()`;
   (e) `camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, &config).await`;
   (f) `camel_bundles::boot(&mut ctx, &config, root).await?` (unchanged);
   (g) route loading through discovery: keep a pre-check that each
   document route file exists (today's `std::fs::metadata` block —
   preserves the explicit missing-file error since glob no-match is
   silent), then build `patterns: Vec<String>` of `root.join(file)`
   display strings from `doc.route_source` (both `RouteFiles` and
   `RouteFilesFromRoot` arms, as today) and call
   `camel_dsl::discover_routes_with_threshold_security_and_env(&patterns, config.stream_caching.threshold, security_ctx, &|name| env.lookup(name))`;
   keep the inline-routes rejection error text for the `Inline` arm;
   delete the per-file `parse_yaml` loop;
   (h) `camel_bundles::security_boot::install_sql_startup_checks(&mut ctx, &defs)`;
   (i) add routes + `ctx.start()` (unchanged).
3. Map `DiscoveryError::Env` to the same unresolved-placeholder
   `CamelError::Config` message shape the current code produces (file
   path + var name + "no layer of the scenario environment defines it").
4. New test file `boot_scenario_test.rs` (wired into `lib.rs` test module
   tree next to the existing `*_test.rs` files), using `tempfile` +
   `toml` dev-deps (already present).

**Tests:**
- `boot_rejects_keycloak_offline`: temp project with Camel.toml declaring
  a minimal `[security.keycloak]` section (use the keycloak test fixture
  from `crates/camel-bundles/src/security_boot.rs` — moved there from
  `crates/camel-cli/src/security.rs` by task 2.2) + one routeFile →
  `boot_scenario` returns Err matching
  `CamelError::AuthProviderUnavailable(_)` (assert variant by
  `matches!`, never by message).
- `boot_rejects_oidc_offline`: same arrange with the minimal
  `[security.oidc]` literal satisfying `OidcSecurityConfig`'s required
  fields (`issuer` is the only required field, camel-config
  config.rs:1389-1396):
  `[security.oidc]\nissuer = "https://oidc.example.test"` → Err matching
  `CamelError::AuthProviderUnavailable(_)`.
- `boot_rejects_wasm_security_policies`: Camel.toml with a minimal
  `[security.policies]` entry → Err `CamelError::Config(_)` whose message
  contains `not supported in the scenario tier`.
- `boot_hermetic_env_not_resolved_from_process`: routeFile with
  `${env:RC_6BSF_HERMETIC}`; `std::env::set_var("RC_6BSF_HERMETIC",
  "leak")` at test start (remove_var at end); LayeredEnv built with empty
  doc/harness/passthrough maps → Err; assert the error names
  `RC_6BSF_HERMETIC` and the route never received "leak" (the error is
  the unresolved-placeholder Config error, not a successful boot).
- `boot_missing_route_file_names_the_file`: document routeFiles lists
  `nope.yaml` → Err `CamelError::Io(_)` whose Display contains `nope.yaml`
  (missing-file semantics preserved through the delegation; the variant
  is pinned — the existing code produces `CamelError::Io`).
- `boot_inline_routes_still_rejected`: document with Inline route source
  → Err with the existing inline-routes message.

**Acceptance:**
- `cargo test -p camel-integration-test` passes (new + existing tests).
- `cargo test -p camel-integration-test --features http` passes (runs the
  `http_partner_test` suite, which has no default-feature coverage —
  existing boot-dependent tests untouched and green).
- `grep -n "parse_yaml" crates/camel-integration-test/src/boot_scenario.rs`
  returns nothing (per-file parsing gone).
- `cargo fmt --check`,
  `cargo clippy -p camel-integration-test --all-features -- -D warnings`
  exit 0.

- [x] 3.1

### camel-cli (scenario runner)

#### Task 3.2: infra-unavailable classification for boot errors

**Files:**
- `crates/camel-cli/src/commands/test/scenario.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified)

**Steps:**
1. In `Cargo.toml`: extend the `integration-http` feature to also enable
   `camel-integration-test/security`.
2. In `scenario.rs` at the `boot_scenario` error site (~lines 404-410):
   classify by variant —
   `let class = matches!(e, CamelError::AuthProviderUnavailable(_)).then_some("infra-unavailable").unwrap_or("full-boot-failure");`
   and format `doc_error: Some(format!("{class}: scenario boot failed: {e}"))`.
   Add a comment citing the variant-classification convention from
   `camel-integration-test/src/document.rs` (classification by variant,
   never message text).

**Tests:**
- `keycloak_boot_reports_infra_unavailable` (in the scenario.rs test
  module, feature `integration-http`): TempProject with Camel.toml
  declaring minimal `[security.keycloak]` + a valid routeFile + a
  minimal scenario document (reuse `write_project` and the doc-shape
  helpers in the same file) → run the scenario document through the
  code path that produces `ScenarioDocResult` → assert
  `doc_error` starts with `"infra-unavailable:"` and `apparatus == true`;
  every other boot failure shape (e.g. a bad routeFile reference)
  still yields `"full-boot-failure:"` (second assertion in the same test
  or a sibling `boot_failure_stays_full_boot_failure`).

**Acceptance:**
- `cargo test -p camel-cli --lib --features integration-http scenario`
  passes including the new test.
- `cargo fmt --check`,
  `cargo clippy -p camel-cli --all-features -- -D warnings` exit 0.

- [x] 3.2

#### Task 3.3: scenario-tier acceptance tests (divergence rows)

**Files:**
- `crates/camel-cli/src/commands/test/scenario.rs` (modified — tests only)

**Steps:**
1. Add tests to the existing `#[cfg(all(test, feature = "integration-http"))]`
   module, each using the `TempProject` + `write_project` pattern:
   write `Camel.toml`, route file(s), and the scenario document; boot
   via `boot_scenario` (directly, like the existing tests do) unless the
   assertion is about document-level classification (then through the
   document runner).
2. Test list (one per divergence row + the parity row):
   - `security_policy_route_boots_in_tier`: Camel.toml with native
     security (one bearer-token credential — the fixture from
     `crates/camel-bundles/src/security_boot.rs` tests, moved there by
     task 2.2), routeFile with a
     `from: direct:sec → security_policy → to: mock:out` route (the
     smallest security_policy fixture from the same file) → boot Ok (the
     pre-fix behavior was a hard compile failure of the default security
     context). Then drive the booted route through the scenario action
     pair the runtime-boot "both callers" scenario requires: send
     `direct:sec` WITH the valid native bearer token → expect on
     `mock:out` receives; send `direct:sec` WITHOUT credentials → the
     action outcome is `CamelError::Unauthenticated` (camel-auth maps
     every native refusal shape to that variant, types.rs:36-38 — the
     same pin task 2.3's run-side test uses).
   - `public_bind_without_ack_fails_closed_in_tier`: Camel.toml with
     `[binds."0.0.0.0:41998"]` WITHOUT `allow_public_exposure` (fixed
     distinctive port, mirroring the run_tests fixture pattern),
     routeFile with an `http:` consumer on that non-loopback bind
     serving a Public exposure → `boot_scenario`/`ctx.start()` returns
     Err `CamelError::RouteError` whose message contains
     `non-loopback address; acknowledge via [binds` (the
     `enforce_bind_exposure_gate` refusal, camel-auth bind_gate.rs:74-79
     — the same pin task 2.3's run-side test uses).
   - `stream_cache_threshold_smoke_boot`: Camel.toml with
     `[stream_caching] threshold = 512` + route with a bare
     `stream_cache:` step → boot Ok (wiring executes; threading proof is
     task 1.1's equality test).
   - `templated_route_file_materializes_in_tier`: routeFile with one
     template + two templated routes from `direct:t1`/`direct:t2` to
     `mock:t1`/`mock:t2` → boot Ok and the scenario document sends
     `direct:t1` → expects on `mock:t1` receives (proves ≥1 materialized
     route; assert both by sending to `direct:t2` as a second action).
   - `sql_dynamic_query_fails_closed_in_tier`: routeFile with a `sql:`
     dynamic-query endpoint shaped like the ADR-0033 fixtures in
     `crates/camel-core/src/startup_validation.rs` tests (~402-437) →
     boot Err at startup with a startup-validation error whose message
     names `sql-dynamic-query` (the ConfigCheck id,
     startup_validation.rs:81).
   - `wasm_security_policies_rejected_in_tier_doc`: Camel.toml with
     minimal `[security.policies]` → document run yields
     `full-boot-failure` doc_error whose message contains the v1 tier
     limitation text (pairs with 3.2's keycloak classification test).
3. Register every test in the module; no production-code edits in this
   task.

**Tests:**
- The six tests above are this task's tests (name, arrange, act, assert
  as listed in the steps).

**Acceptance:**
- `cargo test -p camel-cli --lib --features integration-http scenario`
  passes with the six new tests green.
- Existing scenario tests unmodified and green.
- `cargo fmt --check` and
  `cargo clippy -p camel-cli --all-features -- -D warnings` exit 0.

- [x] 3.3

#### Task 3.4: docs alignment (CONTEXT.md)

**Files:**
- `crates/camel-bundles/CONTEXT.md` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. `camel-bundles/CONTEXT.md`: add the `security_boot` module to the
   crate's owned-surface list (ungated installers + `ensure_security_supported`;
   security-context builder behind the `security` feature; camel-dsl /
   camel-auth / camel-component-keycloak feature deps).
2. `camel-dsl/CONTEXT.md`: add
   `discover_routes_with_threshold_security_and_env` to the discovery
   contract surface with the hermetic-lookup note (process env never
   consulted through this entry).
3. `camel-integration-test/CONTEXT.md`: update the boot_scenario section
   (now delegates to the shared composition root; ordering; offline
   security gating — keycloak infra-unavailable, wasm policies rejected
   v1; `security` feature).
4. Scan `CONTEXT-MAP.md` for the ADR-0069 shared-boot clause references —
   if it tracks per-crate charters, no edit is needed beyond confirming
   nothing contradicts; do not edit ADR-0069.

**Tests:**
- `docs_cite_new_surface`: `rg -n "security_boot" crates/camel-bundles/CONTEXT.md`
  hits ≥1 line; `rg -n "discover_routes_with_threshold_security_and_env" crates/camel-dsl/CONTEXT.md`
  hits ≥1 line; `rg -n "AuthProviderUnavailable|infra-unavailable" crates/camel-integration-test/CONTEXT.md`
  hits ≥1 line.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- The three greps above each return at least one line.

- [x] 3.4
