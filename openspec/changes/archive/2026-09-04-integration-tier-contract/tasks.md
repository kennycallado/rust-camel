# Tasks: integration-tier-contract

Normative sources: ADR-0069, the blessed specs under
`openspec/changes/integration-tier-contract/specs/` (integration-tier,
runtime-boot, mock-testkit delta). Each task lists the scenarios it owns.

Conventions for all Rust tasks: run every cargo command inside the feature
worktree; `Duration` values in documents are humantime-style strings
(`"1s"`, `"50ms"`, precedent: `settle`) deserialized through a custom serde
visitor; every new public enum of the new crates carries `#[non_exhaustive]`
(ADR-0049 posture, verified by `cargo xtask lint-non-exhaustive`).

Line citations of `run.rs` in Phase 1 are indicative anchors against the
file at blessed-hash time: locate the named construct (macro, block,
call) by content, not by absolute line number, and re-verify the
surrounding block before deleting or converting.

## Phase 1: camel-bundles extraction

### camel-bundles

#### Task 1.1: create camel-bundles crate with the extracted bundle cascade

**Files:**
- `crates/camel-bundles/Cargo.toml` (new)
- `crates/camel-bundles/src/lib.rs` (new)
- `crates/camel-bundles/CONTEXT.md` (new)
- `crates/camel-core/src/context.rs` (modified: add the `&mut self`
  `add_lifecycle` seam, step 2)
- `Cargo.toml` (modified: workspace members list)

**Steps:**
1. Create `crates/camel-bundles/Cargo.toml` as a publishable library crate
   depending on `camel-api`, `camel-config`, `camel-component-api`,
   `camel-core` (`languages`, `RegistryComponentContext`), `tokio`
   (async runtime primitives for the shutdown guards), `async-trait`,
   and `toml` (raw config tables for the cascade macro), plus EVERY crate
   the cascade block references: `camel-component-http`,
   `camel-component-ws`, `camel-component-file`,
   `camel-component-container`, `camel-template`, `camel-component-jms`,
   `camel-component-cxf`, `camel-component-controlbus`,
   `camel-component-cron`, `camel-component-direct`,
   `camel-component-log`, `camel-component-mock`, `camel-component-seda`,
   `camel-component-timer`, `camel-component-validator`, `camel-master`,
   `camel-xj`, `camel-xslt`, `camel-component-opensearch`,
   `camel-component-redis`, `camel-component-sql`; optional behind
   features mirroring the current run.rs gates (`run.rs` cfg lines):
   `http-static` (HttpStaticBundle), `kafka` (KafkaBundle), `mqtt`
   (MqttBundle), `surrealdb` (SurrealDbBundle), `grpc` (GrpcBundle),
   `llm` (LlmBundle), `mcp` (McpBundle), `wasm` (WasmBundle).
2. In `src/lib.rs`, define `pub struct BootHandle` owning the teardown
   SEQUENCING for the bridge cleanup and the jms/cxf pools (ADR-0069
   section 10; moved from `run.rs:693-731`). Add `pub fn
   add_lifecycle<L: Lifecycle + 'static>(&mut self, service: L)` to
   `CamelContext` (`crates/camel-core/src/context.rs`; a `&mut self`
   sibling of the consuming builder `with_lifecycle` at line 526 —
   `with_lifecycle` refactors to delegate to it; `CamelContext` has no
   `Default` impl, so `mem::take` is NOT viable and this seam is
   required for the `&mut ctx` boot signature). `BridgeCleanup` stays
   REGISTERED as a context `Lifecycle` (`run.rs:19-43`), registered by
   `boot` through `ctx.add_lifecycle(BridgeCleanup { ... })`; its
   invocation is driven by the handle's shutdown
   ordering. Signatures:
   `pub async fn shutdown(&self, ctx: &CamelContext) -> Result<(), CamelError>`
   delegating to `pub async fn shutdown_with_deadline(&self, ctx:
   &CamelContext, deadline: Duration) -> Result<(), CamelError>` (default
   30s), both preserving the current teardown ordering exactly:
   `jms_pool.begin_shutdown()` → `ctx.stop()` (which drains the
   context-registered lifecycles, BridgeCleanup included) →
   timeout-wrapped `pool.shutdown()`.
3. Define `pub async fn boot(ctx: &mut CamelContext, config: &CamelConfig,
   project_root: &Path) -> Result<BootHandle, CamelError>`: ONE context
   creation path — the CALLER creates and pre-configures the context
   (the CLI keeps its `run.rs:125-159` and `167-307` setup untouched:
   function lifecycle, datasource health wiring, WASM beans, security
   context, bind acknowledgements — while the datasource catalog
   construction at `run.rs:160-165` MOVES into `camel_bundles::boot`,
   using the prepared context's health registry; the harness calls
   `camel_config::configure_context_with_beans` itself the same way the
   CLI does today) and passes the prepared context in; `boot` constructs
   the datasource catalog from the config and passes it via `with_catalog`
   to the Sql and SurrealDb bundles (mirroring `run.rs` sql/surrealdb
   wiring), constructs the `WasmBundle` through `WasmBundle::new` with
   `ctx.registry_arc()` and the wasm root state when the `wasm` feature
   is on (mirroring the run.rs wasm wiring), then registers EVERY
   component in the `run.rs:309-508` block through one
   `register_bundle!` cascade moved verbatim: the gated bundles of step 1
   plus the always-registered bundles (Http, Ws, File, Container,
   Template, Jms, Cxf, OpenSearch, Redis, Sql, Master, Validator, Xj,
   Xslt) and the built-in single components (ControlBus, Cron, Direct,
   Log, Mock, Seda, Timer). Returns the handle only — the context stays
   owned by the caller. `boot` configures and registers only: route
   loading, discovery, startup checks, and `ctx.start()` stay with the
   caller.
4. Feature forwarding: `camel-cli` gains forwarding features for each
   gate of step 1 (`kafka = ["camel-bundles/kafka"]`, and likewise mqtt,
   surrealdb, grpc, llm, mcp, wasm, http-static), preserving current
   default membership EXACTLY: `grpc`, `wasm`, `http-static`, `llm`,
   `surrealdb`, `mqtt`, and `mcp` remain in default features; `kafka`
   remains opt-in. The `exec` feature stays CLI-owned (conditional exec
   guard remains in the CLI).
5. No `std::process::exit` anywhere in this crate; all failures return
   `CamelError::Config` or `CamelError::Component` with the same messages the
   inline cascade produced.
6. Write `crates/camel-bundles/CONTEXT.md` per the repo documentation
   conventions (domain language: boot, bundle cascade, BootHandle; cites
   ADR-0069).

**Tests:** (executable spec)
- `boot_registers_all_bundles_from_fixture_config`: arrange a fixture
  `Camel.toml` enabling http, ws, file, container, template and a context
  prepared via `configure_context_with_beans` → act `boot(&mut ctx,
  &config, &root)` → assert the context resolves components
  `http`, `https`, `ws`, `file`, `container`, `template`, `jms` by name.
  Command: `cargo test -p camel-bundles --lib
  boot_registers_all_bundles_from_fixture_config`. Expected: fails before
  extraction exists.
- `boot_feature_gating_matches_flags`: arrange the same fixture with the
  `kafka` cargo feature disabled → act `boot` → assert resolving `kafka`
  fails with a component-not-found error naming `kafka`; enable the feature
  in a cfg-gated re-run → assert it resolves.
  Command: `cargo test -p camel-bundles --lib boot_feature_gating_matches_flags`.
- `boot_missing_config_key_falls_back_to_bundle_defaults`: arrange a
  `Camel.toml` with no `[http]` table → act `boot` → assert the http bundle
  registers with its serde defaults (same behavior as the inline
  `register_bundle!` fallback path).
  Command: `cargo test -p camel-bundles --lib
  boot_missing_config_key_falls_back_to_bundle_defaults`.

**Acceptance:**
- `cargo build -p camel-bundles` exits 0.
- `cargo clippy -p camel-bundles -- -D warnings` exits 0.
- `cargo test -p camel-bundles --lib` passes.
- `cargo test -p camel-core --lib` passes (the `add_lifecycle` seam and
  the `with_lifecycle` delegation stay green).
- `grep -c 'process::exit' crates/camel-bundles/src/lib.rs` returns 0.
- `cargo test -p camel-cli --lib` passes with the forwarded default
  features (no registration regression).
- The kafka-enabled half of the feature-gating test executes:
  `cargo test -p camel-bundles --features kafka --lib
  boot_feature_gating_matches_flags` exits 0 with the cfg-gated case
  visible in output.

Spec coverage: runtime-boot "identical registration through both boots"
(fixture half), "feature forwarding" (both scenarios).

- [x] 1.1

#### Task 1.2: migrate camel run onto camel_bundles::boot

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified: add camel-bundles dep, forward
  all eight gates per Task 1.1 step 4 while preserving exact default
  membership)

**Steps:**
1. Replace the inline `register_bundle!` cascade and its surrounding
   registration block (`run.rs:309-508`) with a call to
   `camel_bundles::boot(&mut ctx, &camel_config, &project_root)` (the
   CLI's context preparation at `run.rs:125-159` and `167-307` stays
   untouched in the CLI; the catalog block at `160-165` moves per Task
   1.1 step 3).
2. Hold the returned `BootHandle` through the run loop; on exit paths call
   `handle.shutdown(&ctx).await` (ordering per Task 1.1 step 2). The
   jms/cxf pool teardown block at `run.rs:693-731` is deleted from the CLI
   (owned by the handle). Delete ONLY the pool-teardown statements —
   `jms_pool.begin_shutdown()`/`cxf_pool.begin_shutdown()`
   (`run.rs:697-698`), the `ctx.stop()` call (`run.rs:701-704`), and the
   two timeout-wrapped `pool.shutdown()` blocks (`run.rs:707-725`) —
   replacing them with a single `handle.shutdown(&ctx).await`. KEEP the
   shutdown banner (`run.rs:693`), `watcher_shutdown.cancel()`
   (`run.rs:694`), `force_exit.abort()` (`run.rs:727`), and the closing
   `tracing::info!` + `Ok(())` (`run.rs:729-730`).
3. Keep in the CLI: config/override loading (`run.rs:45-123`), route
   discovery/loading and `ctx.start()` (`run.rs:511-605`), watcher, signal
   handling, the second-Ctrl+C path, the conditional exec guard, and
   operator logging (`run.rs:615-691`).
4. Convert the `std::process::exit` sites that lived inside the extracted
   region (`run.rs:140-143`, `run.rs:597-605`) to
   `commands::errors::report_cli_failure_and_exit` calls; the eight exits
   at `run.rs:190-259` are CLI-owned and remain untouched.
5. Delete the now-unused macro and imports from `run.rs`.

**Tests:**
- Named regression guards (existing tests, expected pass after
  migration):
  - `run_exec_guard_test` suite: arrange the exec-guard fixture → act run
    the test target → assert all cases pass (exec guard behavior
    unchanged).
  - `run_empty_discovery_test` suite (target's named cases): arrange a
    route glob matching nothing → act run → assert the run fails naming
    the empty discovery, exit unchanged.
  - `run_watch_test_doc_test` suite: arrange the watch fixture → act run
    the target → assert watch/reload behavior unchanged.
  Commands: `cargo test -p camel-cli --test run_exec_guard_test`,
  `cargo test -p camel-cli --test run_empty_discovery_test`,
  `cargo test -p camel-cli --test run_watch_test_doc_test`.

**Acceptance:**
- `grep -c 'register_bundle' crates/camel-cli/src/commands/run.rs` returns 0.
- `grep -c 'camel_bundles::boot' crates/camel-cli/src/commands/run.rs`
  returns at least 1.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- The three run test targets above pass.

Spec coverage: runtime-boot "BootHandle lifecycle" — "CLI ownership
unchanged" scenario.

- [x] 1.2

#### Task 1.3: parity and teardown tests for the extracted cascade

**Files:**
- `crates/camel-bundles/tests/parity_test.rs` (new)
- `crates/camel-bundles/tests/fixtures/parity/` (new: `Camel.toml`,
  `routes/http-loop.yaml`)

**Steps:**
1. Add a parity fixture `Camel.toml` configuring http (explicit loopback
   port), file, container, and template.
2. Write the parity test asserting two consecutive `boot` calls with the
   same fixture produce contexts with identical registered component name
   sets (sorted Vec comparison) and identical per-bundle config keys.
3. Write the teardown test asserting `BootHandle::shutdown(&ctx)` after a
   boot closes pools such that a second boot on the same fixture succeeds
   (no leaked exclusive resources).
4. Assert no tracing double-init panic across two consecutive boots by
   calling `boot` twice in one test process (regression for the
   tracing-subscriber hazard, ADR-0069 record).

**Tests:** (exact test names; cargo filters are substrings of these)
- `two_boots_register_identical_sets`: fixture → boot twice → assert equal
  sorted component-name sets. Command: `cargo test -p camel-bundles
  --test parity_test two_boots_register_identical_sets`.
- `shutdown_then_boot_succeeds`: fixture → boot → `shutdown(&ctx)` → boot
  again → assert the second boot returns Ok. Command:
  `cargo test -p camel-bundles --test parity_test shutdown_then_boot_succeeds`.
- `consecutive_boots_do_not_panic_tracing`: call
   `configure_context_with_beans` then `boot` twice in one process
   (mirroring the CLI's real double-boot on watch reload) → assert no
   tracing double-init panic. `init_tracing_subscriber` guards its
   install with `try_init()` (`context_ext.rs:853`), so the double call
   takes the warning path by construction; this test locks that guard in
   (it fails if anyone swaps the install to `set_global_default`-style
   panicking init). Arrange deps: `camel-config` (feature `test-util`
   if gated), explicit loopback-port fixture shared with the parity
   tests; keep both boots up concurrently, then shut both down → assert
   no panic and clean shutdown.
  Command: `cargo test -p camel-bundles --test parity_test
  consecutive_boots_do_not_panic_tracing`.

**Acceptance:**
- `cargo test -p camel-bundles --test parity_test` passes (verify the three
  test names appear in the output — zero filtered runs).
- `cargo fmt --check --all` exits 0.

Spec coverage: runtime-boot "identical registration through both boots"
(both-boot structural half), "explicit teardown" scenario.

- [x] 1.3

## Phase 2: runner and filters

### camel-integration-test

#### Task 2.1: scenario document model and parser with vocabulary ban

**Files:**
- `crates/camel-integration-test/Cargo.toml` (new)
- `crates/camel-integration-test/src/lib.rs` (new)
- `crates/camel-integration-test/src/document.rs` (new)
- `crates/camel-integration-test/CONTEXT.md` (new)
- `Cargo.toml` (modified: workspace members)

**Steps:**
1. Create the publishable crate depending on `serde`, `serde_yaml`,
   `humantime` (Duration strings), `futures` (BoxFuture in the adapter
   trait), `camel-api`, `camel-dsl` (route file
   parsing reuse), and `camel-core` (`RouteDefinition` and
   `InterceptAction::{SkipTo, DivertCopyTo}` from
   `camel-core/src/intercept.rs` — camel-core types not re-exported by
   camel-dsl; ADR-0069 section 10 permits this dependency direction).
2. In `document.rs`, define `pub struct ScenarioDocument` with fields:
   `route_source: RouteSource` (enum `RouteFiles(Vec<PathBuf)>` /
   `RouteFilesFromRoot(Vec<PathBuf>)` / `Inline(Vec<RouteDefinition>)`),
   `scenario: Vec<ScenarioAction>`, `env: Option<BTreeMap<String, String>>`,
   `env_passthrough: Option<Vec<String>>`,
   `profile: Option<String>`.
3. Define `pub enum ScenarioAction` variants `Send { to: EndpointRef,
   body: Option<Value>, headers: Option<BTreeMap<String, Value>> }`,
   `Receive { from: EndpointRef, deadline: Duration, extract:
   Option<BTreeMap<String, String>> }`, `Sleep { duration: Duration }`,
   `Validate { target: ScenarioTarget, expectation: Expectation }`.
   `EndpointRef` deserializes from a bare string (shorthand:
   `"direct:start"` → `EndpointRef { endpoint: "direct:start",
   provisioning: None, bind_var: None }`) or a map with keys `endpoint`
   (required), `provisioning`, `bindVar`. `ScenarioTarget` is
   `LastReceived(EndpointRef)` or `Variable(String)` (validates an
   extracted scenario variable). `Expectation` reuses matcher grammar
   keys (`equals`, `regex`, `contains`, `startsWith`, `endsWith`,
   `exists`, `jsonSubset`) mirroring the mock-testkit matcher rules.
   `bind_var` is the variable name the harness will inject for this
   endpoint's bound address when `provisioning` is `harness`
   (e.g. `bindVar: PARTNER`).
4. Parse with unknown-field rejection (serde deny_unknown_fields) and a
   `parse_scenario_document(path) -> Result<ScenarioDocument, DocError>`
   that rejects: zero or multiple route sources (same messages as the
   unit-tier parser), a `Receive` without `deadline`, any document also
   carrying unit-tier keys (`inputs`, `expects`, `intercepts`) with error
   variant `DocError::MixedVocabulary`, and a document `env` entry whose
   key equals any endpoint's `bind_var` with error variant
   `DocError::ReservedEnvKey` naming the key and endpoint (the rule is
   static and document-derivable: the reserved set is exactly the
   `bind_var` values declared by the document's own endpoints).
5. Provisioning gate: the parser accepts only `harness` (or absent) in
   `EndpointRef.provisioning`; `testcontainer` and `user-provided` fail
   with `DocError::UnsupportedProvisioning` naming the value and endpoint
   (CLI surfaces it as `infra-unavailable`-class exit 2).
6. The parser accepts both `.test.yaml` and `.test.yml` through the
   reserved-suffix predicate `camel_dsl::discovery::is_test_document`.
7. Write `crates/camel-integration-test/CONTEXT.md` (domain language:
   scenario, action, partner, tier; cites ADR-0069).

**Tests:**
- `mixed_vocabulary_rejected`: arrange YAML with `scenario:` and `inputs:`
  → act `parse_scenario_document` → assert `Err(DocError::MixedVocabulary)`
  and the error string contains `doc-validation`.
  Command: `cargo test -p camel-integration-test --lib
  mixed_vocabulary_rejected`.
- `scenario_with_expects_rejected`: arrange `scenario:` plus `expects:` →
  act parse → assert `DocError::MixedVocabulary`.
  Command: `cargo test -p camel-integration-test --lib
  scenario_with_expects_rejected`.
- `receive_without_deadline_rejected`: arrange a `receive` action with no
  `deadline` key → act parse → assert `DocError::Validation` naming the
  action index.
  Command: `cargo test -p camel-integration-test --lib
  receive_without_deadline_rejected`.
- `scenario_with_env_accepted`: arrange `scenario:` plus `env: {HTTP_PORT:
  "18080"}` → act parse → assert `Ok` with the env map present.
  Command: `cargo test -p camel-integration-test --lib
  scenario_with_env_accepted`.
- `reserved_provisioning_rejected`: arrange a send action endpoint with
  `provisioning: testcontainer` (and a second case `user-provided`) → act
  parse → assert `DocError::UnsupportedProvisioning` naming the value.
  Command: `cargo test -p camel-integration-test --lib
  reserved_provisioning_rejected`.
- `reserved_env_key_rejected`: arrange an endpoint with
  `provisioning: harness, bindVar: PARTNER` and a document `env` entry
  `PARTNER: http://127.0.0.1:9999` → act parse → assert
  `DocError::ReservedEnvKey` naming `PARTNER` and the endpoint.
  Command: `cargo test -p camel-integration-test --lib
  reserved_env_key_rejected`.

**Acceptance:**
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0.
- `cargo test -p camel-integration-test --lib` passes (six named tests
  visible in output).

Spec coverage: mock-testkit parsing delta — "mixed vocabulary rejected at
load", "scenario document with unit-tier fields rejected",
"scenario-only document with env accepted"; integration-tier actions —
"missing deadline is a load error"; activation — "reserved provisioning
value rejected".

- [x] 2.1

#### Task 2.2: pure tier derivation function

**Files:**
- `crates/camel-integration-test/src/tier.rs` (new)
- `crates/camel-integration-test/src/lib.rs` (modified: export module)

**Steps:**
1. Define `pub enum Tier { Lean, Full }` and
   `pub fn derive_tier(routes: &[RouteDefinition], doc: &DocumentInputs)
   -> Tier` where `DocumentInputs` carries `has_scenario: bool`,
   `intercepts: &[(String, InterceptAction)]` (sourced from the
   unit-tier document model — `parse_test_document` output, NOT the
   scenario parser, which bans `intercepts`; `derive_tier` serves both
   tiers' documents)
   (`InterceptAction` re-exported from camel-core), and
   `unit_schemes: &[String]` (schemes named by `inputs`/`expects`).
2. `has_scenario == true` returns `Tier::Full` unconditionally.
3. Otherwise compute the scheme closure: walk every `RouteDefinition`
   recursively through nested steps collecting endpoint schemes from
   from/to and any step carrying a URI; subtract endpoints whose exact URI
   string matches an intercept with action `SkipTo` (verbatim key match,
   query parameters significant — mirror `parse_test_document` matching);
   add `unit_schemes`.
4. The closure contains any scheme outside `{direct, log, mock, seda,
   timer}`, OR any endpoint whose scheme position contains a placeholder
   (`${` or `{{` before the first `:`), OR any dynamic-dispatch step kind
   (`recipient_list`, `routing_slip`/`routingSlip`, `dynamic_router`,
   `to_d`/`toD`) found anywhere in the recursive walk → `Tier::Full`.
5. Else `Tier::Lean`. The function is pure: no I/O, no env reads, no
   clocks.

**Tests:** (exact names)
- `tier_lean_document_stays_lean`, `tier_skipto_subtracts_from_closure`,
  `tier_divertcopyto_does_not_subtract`,
  `tier_placeholder_in_scheme_forces_full`,
  `tier_dynamic_dispatch_forces_full` (parameterized over recipient_list,
  routing_slip, dynamic_router, to_d),
  `tier_scenario_section_forces_full`, `tier_all_route_sources_count`
  (inline and routeFilesFromRoot halves). Bodies follow the blessed
  scenarios: e.g. `tier_divertcopyto_does_not_subtract` arranges a route
  `to kafka:orders` plus a `DivertCopyTo` intercept → asserts `Tier::Full`.
  Command (all): `cargo test -p camel-integration-test --lib tier_`.

**Acceptance:**
- All seven test functions above pass and appear by name in the output
  (the dynamic-dispatch one parameterized over four kinds).
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0.

Spec coverage: integration-tier "Pure tier derivation" — all seven
scenarios.

- [x] 2.2

#### Task 2.3: layered hermetic environment source

**Files:**
- `crates/camel-integration-test/src/env_layers.rs` (new)
- `crates/camel-integration-test/src/lib.rs` (modified: export module)
- `crates/camel-config/src/config.rs` (modified: lookup-injectable loader
  seam)
- `crates/camel-dsl/src/env_interpolation.rs` (modified: lookup variant)

**Steps:**
1. Define `pub struct LayeredEnv { doc: BTreeMap<String, String>,
   harness_provisioned: BTreeMap<String, String>,
   passthrough: Vec<String>,
   ambient: Arc<dyn Fn(&str) -> Option<String> + Send + Sync> }` with a
   public cross-crate constructor `pub fn new(doc: BTreeMap<String,
   String>, harness_provisioned: BTreeMap<String, String>, passthrough:
   Vec<String>, ambient: Arc<dyn Fn(&str) -> Option<String> + Send +
   Sync>) -> Self` (and `pub fn ambient_std() -> Arc<dyn Fn(&str) ->
   Option<String> + Send + Sync>` wiring `std::env::var` for production
   callers).
   `pub fn lookup(&self, key: &str) -> Option<String>` implements the
   precedence: harness-provisioned bindings first (exactly the `bind_var`
   keys of `provisioning: harness` endpoints, which the parser has
   already guaranteed are absent from `doc` via
   `DocError::ReservedEnvKey`, Task 2.1); then document `env`; then the
   injected ambient lookup result iff `key` is listed in `passthrough`;
   else None. NOTHING in this crate calls `std::env::set_var`.
2. Loader seam reaching the production call chain: add
   `interpolate_env_with(input: &str, lookup: &dyn Fn(&str) ->
   Option<String>)` in `camel-dsl/src/env_interpolation.rs` (existing
   `interpolate_env` delegates with a `std::env::var` closure — behavior
   unchanged). In `camel-config/src/config.rs`, add lookup-accepting
   variants of `resolve_strict_leaf`/`resolve_plain_leaf` and
   `resolve_tree_with(root, &lookup)`, plus a public
   `from_file_with_env_and_lookup(path, profile: Option<&str>, lookup)`
   entry (explicit profile parameter) that threads through
   `build_from_toml_value_inner` to the `resolve_tree_placeholders`
   call site (`config.rs:2892`). Existing `from_file_with_env` delegates
   with the ambient closure and ambient profile (behavior unchanged for
   `camel run`).
3. Profile pinning: reuse `from_file_async_with_profile`'s existing
   explicit-profile-over-ambient behavior (`config.rs:2485-2487`); the
   scenario path passes the document's `profile` (default `default`)
   explicitly. Ambient `CAMEL_PROFILE` is ignored for scenario documents.
4. Unresolved `${env:NAME}` under the layered lookup fails with the same
   error shape as today's resolver, naming the variable and the document
   field.

**Tests:**
- `env_document_value_wins_over_unlisted_ambient`: construct LayeredEnv
  with doc HTTP_PORT=18080 and an injected ambient map containing
  HTTP_PORT=8080 (not allowlisted) → assert lookup returns "18080".
  Command: `cargo test -p camel-integration-test --lib
  env_document_value_wins_over_unlisted_ambient`.
- `env_unlisted_ambient_is_invisible`: doc without the key, injected
  ambient has NOPE set → assert lookup returns None and
  `resolve_tree_with` on a leaf `${env:NOPE}` errors naming `NOPE`.
  Command: `cargo test -p camel-integration-test --lib
  env_unlisted_ambient_is_invisible`.
- `env_allowlisted_passthrough_visible`: key present in the document's
  `env_passthrough`, injected ambient has the value → assert lookup
  returns the ambient value.
  Command: `cargo test -p camel-integration-test --lib
  env_allowlisted_passthrough_visible`.
- `camel_config_resolution_unchanged_for_run`: existing camel-config and
  camel-dsl placeholder tests pass unchanged. Command:
  `cargo test -p camel-config --lib && cargo test -p camel-dsl --lib`.

**Acceptance:**
- Tests above pass; `cargo test -p camel-config --lib` green.
- `grep -rn 'set_var' crates/camel-integration-test/src` returns 0.

Spec coverage: integration-tier "Layered hermetic environment" — both
scenarios; mock-testkit parsing delta env sentence.

- [x] 2.3

#### Task 2.4: scenario action runner with typed fake adapters

**Files:**
- `crates/camel-integration-test/src/runner.rs` (new)
- `crates/camel-integration-test/src/adapters.rs` (new)
- `crates/camel-integration-test/src/lib.rs` (modified)

**Steps:**
1. Define `pub trait PartnerAdapter: Send + Sync { fn send<'a>(&'a self,
   target: &'a EndpointRef, msg: OutgoingMessage) -> BoxFuture<'a,
   Result<(), TransportError>>; fn receive<'a>(&'a self, source: &'a
   EndpointRef, deadline: Duration) -> BoxFuture<'a,
   Result<IncomingMessage, ReceiveTimeout>>; }` implemented by
   `FakeAdapter` (in-memory mpsc with recorded sent messages and a
   scriptable incoming queue).
2. Define the runner: `pub async fn run_scenario(doc:
   &ScenarioDocument, router: &PartnerRouter, vars: &mut
   ScenarioVars) -> Result<ScenarioVerdict, ScenarioFailure>` where
   `PartnerRouter` (defined in `adapters.rs`) implements
   `PartnerAdapter` and dispatches by `EndpointRef` equality to the
   endpoint-keyed adapter map it wraps (`pub struct PartnerRouter {
   adapters: BTreeMap<String, Box<dyn PartnerAdapter>> }`, constructor
   `pub fn new(adapters: BTreeMap<String, Box<dyn PartnerAdapter>>)`).
   The runner executes
   actions in order: `Send` dispatches through the adapter; `Receive`
   awaits with the deadline and applies `extract` into `ScenarioVars`;
   `Sleep` uses tokio time; `Validate` evaluates the matcher grammar
   against the last received message or an extracted variable.
3. Define `ScenarioFailure` with variants `ReceiveTimeout`,
   `ValidationMismatch { action: usize, detail: String }`,
   `VarUnresolved { name: String }` (verdict class) and
   `ActionTransport { action: usize, source: TransportError }`,
   `PartnerStartup`, `ShutdownFailure` (apparatus class).
4. Every adapter call takes the deadline from the action (receive) or a
   bounded default (send: 30s); no unbounded awaits exist in the runner.

**Tests:** (exact names; each test wraps its `FakeAdapter` in a
single-entry `PartnerRouter`)
- `send_then_receive_within_deadline`: FakeAdapter scripted to echo →
  scenario send+receive("1s")+validate equals → assert
  `Ok(ScenarioVerdict::Pass)`.
  Command: `cargo test -p camel-integration-test --lib
  send_then_receive_within_deadline`.
- `receive_timeout_is_verdict_failure`: FakeAdapter with empty queue →
  receive with "50ms" deadline → assert `Err(ScenarioFailure::ReceiveTimeout)`.
  Command: `cargo test -p camel-integration-test --lib
  receive_timeout_is_verdict_failure`.
- `variable_extraction_flows_forward`: receive extracts header `X-Id`
  into var `id`; later validate `equals` on the var → assert pass when the
  fake sends the matching header, `ValidationMismatch` when not.
  Command: `cargo test -p camel-integration-test --lib
  variable_extraction_flows_forward`.
- `transport_error_is_apparatus_failure`: FakeAdapter configured to fail
  send → assert `Err(ScenarioFailure::ActionTransport { action: 0, .. })`.
  Command: `cargo test -p camel-integration-test --lib
  transport_error_is_apparatus_failure`.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` passes.
- `grep -c 'unbounded' crates/camel-integration-test/src/runner.rs`
  returns 0.

Spec coverage: integration-tier "Ordered scenario actions" — all three
scenarios; taxonomy unit half of "receive timeout is a verdict failure".

- [x] 2.4

### camel-cli

#### Task 2.5: tier filters, tier report, and taxonomy exit mapping in camel test

**Files:**
- `crates/camel-cli/src/commands/test.rs` (modified)
- `crates/camel-cli/src/commands/test/filters.rs` (new)
- `crates/camel-cli/src/commands/test/document.rs` (modified:
  scenario-document dispatch alongside `parse_test_document`)
- `crates/camel-cli/src/commands/test/runner.rs` (modified: scenario
  execution path and exit mapping)
- `crates/camel-cli/src/commands/test/junit.rs` (modified: tier property)
- `crates/camel-cli/Cargo.toml` (modified: dep on camel-integration-test)
- `crates/camel-cli/tests/test_tier_filters.rs` (new)

**Steps:**
1. Add `--unit` and `--integration` boolean flags to the `camel test`
   argument parser; reject both together with a misuse error and exit 2
   before any document is read.
2. In the expansion path, apply the tier filter by derived tier: documents
   admitted through directory expansion that do not match the selected
   tier are excluded silently (no stdout, no junit rows); a document named
   explicitly on the command line that does not match fails with the class
   `tier-filter-collision` and exit 2.
3. Compose tier filters AND with the existing `--filter-file` /
   `--filter-endpoint` survivors (all kinds apply; repeats of one kind are
   OR, unchanged).
4. Emit the tier annotation (`lean`/`full`) per document in the stdout
   line and as a junit `<property name="tier">` row; map scenario
   verdict-class failures to exit 1 and apparatus-class to exit 2 under
   the existing precedence (2 > 1 > 0); a `shutdown-failure` after a
   recorded verdict reports both, exit 2. LOAD-TIME ADAPTER COVERAGE
   (carry-forward, Task 2.4 review): before `run_scenario`, verify every
   `Send.to`/`Receive.from` endpoint of the doc is covered by the
   harness-built adapter map; an uncovered endpoint is a harness wiring
   error → exit 2 (doc-validation class), never a silent
   `ReceiveTimeout` verdict failure.
5. Phase-2 CLI scope (FakeAdapter smoke): scenario documents parse,
   derive tier, and run against an in-memory adapter when they declare no
   endpoint needing a real transport; documents declaring real transport
   endpoints (e.g. http, when the CLI is built without the
   integration-http forwarding feature from Task 3.1) report
   `infra-unavailable` naming the adapter.

**Tests:** (exact names; CLI-level, exercising the real command path)
- `unit_filter_excludes_full_silently`: temp dir with one lean and one
  full (scenario) doc → run `camel test --unit <dir>` → assert only the
  lean doc's lines appear and exit 0.
- `explicit_full_collides_under_unit`: run `camel test --unit <full-doc>`
  → assert stderr contains `tier-filter-collision` and exit code 2.
- `both_flags_misuse`: run `camel test --unit --integration` → assert exit
  2 with no document read.
- `tier_annotation_in_output`: run one lean doc unfiltered → assert its
  stdout line carries `[lean]`.
- `tier_filter_composes_with_file_filter`: lean+full docs where one lean
  doc matches glob `sub/**` → run `camel test . --unit --filter-file
  'sub/**'` → assert only that doc runs.
- `no_filter_runs_everything_at_derived_tier`: one lean + one FakeAdapter
  scenario doc → run `camel test .` → assert both execute and report
  their tiers.
- `scenario_receive_timeout_exits_1`: FakeAdapter scenario doc with a
  receive that times out → assert the action line reports
  `receive-timeout` and exit code is 1.
- `apparatus_failure_keeps_precedence`: one doc with a failing
  expectation plus one doc failing `infra-unavailable` → assert both
  reported and exit 2.
  Command (all): `cargo test -p camel-cli --test test_tier_filters`.

**Acceptance:**
- `cargo test -p camel-cli --test test_tier_filters` passes with all
  eight names visible in output.
- `cargo test -p camel-cli` existing suite stays green.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.

Spec coverage: mock-testkit filters delta — all five new scenarios; exit
delta — "tier annotation appears per document", "apparatus failure keeps
precedence over verdict failure", "scenario receive timeout exits 1";
integration-tier filters scenarios; taxonomy "infra-unavailable fails
named, never hangs".

- [x] 2.5

## Phase 3: HTTP activation

### camel-integration-test

#### Task 3.1: HTTP partner adapter behind the http feature

**Files:**
- `crates/camel-integration-test/src/adapters/http.rs` (new)
- `crates/camel-integration-test/src/adapters.rs` (modified: module gate)
- `crates/camel-integration-test/Cargo.toml` (modified: `http` feature)
- `crates/camel-cli/Cargo.toml` (modified: forwarding feature
  `integration-http = ["camel-integration-test/http"]`, NOT in defaults)

**Steps:**
0. TRAIT WIDENING FIRST (inter-phase review finding, before any
   HttpPartner code): widen `PartnerAdapter::receive`'s error type from
   bare `ReceiveTimeout` to a small `#[non_exhaustive]` enum
   (`Timeout(ReceiveTimeout)` | `Transport(TransportError)`) so
   mid-scenario receive transport failures are expressible; map
   `Transport` to `ScenarioFailure::ActionTransport` (exit-2 class) in
   the runner's receive path, keep `Timeout` → verdict-class
   `receive-timeout`. Update FakeAdapter + existing tests accordingly.
1. Implement `PartnerAdapter` for `HttpPartner`: constructor binds a
   listener on `127.0.0.1:0` (outbound role) exposing
   `pub fn bound_addr(&self) -> SocketAddr`; scripted responses with
   matchers on path/method; records incoming requests (method, path,
   headers, body bytes).
2. Implement the client role (inbound): `send` performs a real HTTP
   request to a configured address; `receive` awaits the response bounded
   by the action deadline and returns status, headers, body.
3. Gate the module and feature; without `http`, scenario endpoints of
   scheme http keep the `infra-unavailable` path from Task 2.5. The CLI
   forwarding feature `integration-http` (non-default) enables
   `camel test` to run http scenarios when built with it.
4. Listener uses `:0` only — no `find_free_port` probing anywhere.

**Tests:**
- `outbound_partner_records_wire_request`: start HttpPartner, send action
  to `http://127.0.0.1:{bound}/orders` with headers and body → assert the
  recorded request carries the exact headers and bytes.
  Command: `cargo test -p camel-integration-test --features http --lib
  outbound_partner_records_wire_request`.
- `inbound_client_receives_status_headers_body`: local listener serving a
  canned response → send action → assert the response object exposes
  status, headers, body for validation.
  Command: `cargo test -p camel-integration-test --features http --lib
  inbound_client_receives_status_headers_body`.
- Feature-gate compile check: `cargo check -p camel-integration-test`
  (no features) exits 0 with no http partner symbols referenced.

**Acceptance:**
- `cargo test -p camel-integration-test --features http --lib` passes.
- `cargo check -p camel-integration-test` (no features) exits 0.
- `grep -rn 'find_free_port' crates/camel-integration-test/src` returns 0.

Spec coverage: integration-tier "Demand-gated activation" — "http
scenarios isolated behind their feature"; partner-side wire mechanics.

- [x] 3.1

#### Task 3.2: outbound HTTP bridge end-to-end scenario

**Files:**
- `crates/camel-integration-test/src/boot_scenario.rs` (new)
- `crates/camel-integration-test/src/adapters/http.rs` (modified:
  arrival-queue bridge, review amendment)
- `crates/camel-integration-test/src/runner.rs` (modified: selector
  grammar + direct-context sends, review amendment)
- `crates/camel-integration-test/tests/http_outbound_test.rs` (new)
- `crates/camel-integration-test/tests/fixtures/outbound/` (new:
  `Camel.toml`, `routes/bridge.yaml`, `bridge.test.yaml`)

**Steps:**
1. Add `pub struct ScenarioRun { pub ctx: CamelContext, pub boot:
   BootHandle }` and `pub async fn boot_scenario(doc: &ScenarioDocument,
   root: &Path, env: &LayeredEnv) -> Result<ScenarioRun, CamelError>` in
   `boot_scenario.rs`: load config through `from_file_with_env_and_lookup`
   (Task 2.3, with the doc's pinned profile), prepare the context from
   that config through `configure_context_with_beans`, then call
   `camel_bundles::boot(&mut ctx, ...)`, then load the doc's
   route source through the same per-file YAML parser `camel run`
   uses, then `ctx.start()`. HERMETICITY SEAL (review carry-forward,
   Task 2.3 findings 1-2): the config entry used here MUST be a sealed
   variant — profile passed as `Some(doc.profile)` ALWAYS (never None;
   ambient `CAMEL_PROFILE` must not leak), and the `merge_env=true`
   allowlist loop (CAMEL_* overrides reading real env at config load,
   config.rs merge loop) MUST be off or lookup-gated for the scenario
   path. Extend camel-config with the sealed entry (e.g. a
   `merge_env = false` path or lookup-gated merge) in this task if no
   suitable entry exists; a named test must assert an ambient
   `CAMEL_LOG_LEVEL` (or equivalent allowlisted var) set in the test
   process does NOT reach the scenario's loaded config. Partners are NOT owned here: the caller
   constructs them before boot (bind `:0`), builds the harness-provisioned
   map, and passes the resulting `LayeredEnv` in; the caller tears the
   partners down after `handle.shutdown(&ctx)`.    Binding waits at
   `ctx.start()` through the operator readiness signal (rc-w1u9 shipped
   behavior).
1b. LIBRARY-LEVEL DOCUMENT EXECUTION (inter-phase review finding 2 —
   the CLI must stop reconstructing documents): add
   `pub struct DocumentOutcome { pub per_action: Vec<Result<
   ScenarioVerdict, ScenarioFailure>>, pub verdict: Option<
   ScenarioVerdict>, pub final_failure: Option<ScenarioFailure> }` and
   `pub async fn run_scenario_document(doc: &ScenarioDocument, router:
   &PartnerRouter, vars: &mut ScenarioVars) -> DocumentOutcome` in the
   runner module: executes actions in order, records per-action
   outcomes, stops at first failure, and leaves a slot for a
   post-verdict `ShutdownFailure` (appended by the CLI after
   `handle.shutdown`). The existing `run_scenario` stays as the
   single-action primitive both build on. Task 3.5 deletes the CLI-side
   synthetic-document loop (`camel-cli test/runner.rs:
   run_scenario_doc`) and calls `run_scenario_document` instead.
2. Fixture route: `direct:start` → set headers → `to:
   ${env:PARTNER}/orders`. The scenario's partner endpoint declares
   `provisioning: harness, bindVar: PARTNER`; the document does NOT
   declare `PARTNER` in `env` (the parser rejects that, Task 2.1): the
   test harness injects it into the harness-provisioned tier
   (`PARTNER=http://127.0.0.1:<bound>`) after `HttpPartner` binds `:0`.
3. Fixture scenario doc: send a body to `direct:start` (route stimulus),
   receive on the partner endpoint with deadline "2s", validate body,
   headers, and status at the wire. REVIEW AMENDMENTS (inter-phase +
   3.1 reviews, mandatory): (a) ARRIVAL QUEUE — `HttpPartner::serve`
   currently records arrivals in `HttpRecorder` only, unreachable from
   `receive` (which parks in `in_flight` for the client role); add an
   outbound arrival queue (listener → per-endpoint channel → `receive`
   maps arrival into `IncomingMessage`); reconsider the one-in-flight
   v1 bound while redesigning this structure (per-endpoint queue or a
   doc-validation send/receive pairing rule — pick one, document it).
   (b) SELECTOR GRAMMAR — `select_from` needs a `status` head and
   ASCII-case-insensitive header lookup (hyper lowercases; FakeAdapter
   preserves author casing — same selector must behave identically per
   adapter); wire-recording stays lowercase, documented. Amended during
   implementation (sanctioned by review): scalar heads `method` and
   `path` also added — required so outbound arrivals can validate the
   request line (amendment d). (c) ROUTE
   STIMULUS — a `send` addressed to a CONTEXT component (`direct:`)
   must reach the booted SUT, not the partner router: deliver it
   through the booted context's producer path (study how camel-test /
   camel run stimulate routes; reuse that mechanism); partner-scheme
   sends keep going through `PartnerRouter`. (d) The OUTBOUND fixture
   validates method/path/headers/body on ARRIVALS (requests carry no
   status; the scripted response status is harness-known — status
   validation lands in Task 3.3 inbound).
4. Test harness: construct the partners first (`HttpPartner` binds `:0`),
   wrap them in a `PartnerRouter` keyed by endpoint, build the
   harness-provisioned map (`PARTNER=http://127.0.0.1:<bound>`) into a
   `LayeredEnv::new(...)`, call `boot_scenario(doc, root, &env)`, run
   `run_scenario(doc, &router, &mut vars)`, assert pass; a negative
   variant corrupts one header and asserts `ValidationMismatch`
   (regression shape of rc-eoft).
5. Shutdown fault injection, deterministic: register a test-only context
   `Lifecycle` whose `stop()` returns `Err` (a failing teardown
   dependency), run a passing verdict, then `handle.shutdown(&ctx)` →
   assert `ShutdownFailure` is reported deterministically AND the verdict
   stays recorded (exit path 2 at the CLI mapping). Teardown for the
   normal variant calls `handle.shutdown(&ctx)` and asserts clean
   completion.

**Tests:**
- `outbound_bridge_validates_wire`: as above → assert pass.
  Command: `cargo test -p camel-integration-test --features http --test
  http_outbound_test outbound_bridge_validates_wire`.
- `outbound_bridge_header_corruption_fails`: variant with corrupted
  header → assert `ValidationMismatch` naming the header.
  Command: `cargo test -p camel-integration-test --features http --test
  http_outbound_test outbound_bridge_header_corruption_fails`.
- `shutdown_failure_does_not_mask_verdict`: arrange a test-only context
  `Lifecycle` whose `stop()` returns `Err`, run a passing verdict, then
  `handle.shutdown(&ctx)` → assert `ShutdownFailure` reported
  deterministically AND the verdict preserved.
  Command: `cargo test -p camel-integration-test --features http --test
  http_outbound_test shutdown_failure_does_not_mask_verdict`.

**Acceptance:**
- All three tests pass; `cargo fmt --check --all` exits 0.

Spec coverage: integration-tier "Partner-side normative proof" —
"outbound wire validation"; taxonomy "shutdown failure does not mask the
verdict" (fault-injected half).

- [x] 3.2

#### Task 3.3: inbound HTTP consumer end-to-end scenario

**Files:**
- `crates/camel-integration-test/tests/http_inbound_test.rs` (new)
- `crates/camel-integration-test/tests/fixtures/inbound/` (new:
  `Camel.toml`, `routes/consumer.yaml`, `consumer.test.yaml`)

**Steps:**
1. Fixture route: `from: http://127.0.0.1:${env:PORT:-18180}/in` → set
   response status/headers/body → consumer completes.
2. Scenario doc: harness client send action to the consumer address,
   receive the response within deadline "2s", validate status, headers,
   body.
3. Boot through `boot_scenario` (Task 3.2); the test asserts the client
   connects immediately after `boot_scenario` returns, with no sleeps
   (binding waits at `ctx.start()` through the operator readiness signal).
4. Negative variant: mismatching body expectation asserts
   `ValidationMismatch` at the wire.

**Tests:**
- `inbound_consumer_honest_readiness`: boot_scenario then immediate
  connect → assert connection succeeds with zero sleep calls in the test
  body.
  Command: `cargo test -p camel-integration-test --features http --test
  http_inbound_test inbound_consumer_honest_readiness`.
- `inbound_response_validated_on_wire`: full status/headers/body
  validation pass variant and mismatch-fail variant → assert both.
  Command: `cargo test -p camel-integration-test --features http --test
  http_inbound_test inbound_response_validated_on_wire`.

**Acceptance:**
- Both tests pass.
- `grep -c 'sleep' crates/camel-integration-test/tests/http_inbound_test.rs`
  returns 0.

Spec coverage: integration-tier "Partner-side normative proof" — "inbound
readiness is honest", "inbound response validated on the wire"; and, via
`boot_scenario` + routes + `ctx.start()`, the harness half of mock-testkit
"full document boots the shared cascade in-process" (CLI half in Task
3.5).

- [x] 3.3

### CI

#### Task 3.4: integration-http CI job with path filters

**Files:**
- `.github/workflows/integration-http.yml` (new)

**Steps:**
1. New job `integration-http`: triggers on pull_request with paths
   filtering to `crates/camel-integration-test/**`,
   `crates/camel-bundles/**`, `crates/camel-cli/src/commands/**`,
   `crates/components/camel-http/**`, and
   `.github/workflows/integration-http.yml`.
2. Steps: checkout, Rust toolchain matching the existing CI setup, then
   `cargo test -p camel-integration-test --features http`,
   `cargo test -p camel-bundles --features kafka`,
   `cargo test -p camel-cli --features integration-http --test
   test_tier_filters`, and `cargo test -p camel-bundles`.
3. No `#[ignore]` markers on the loopback tests (verify: none added in
   Phase 2/3 files).
4. Default suite untouched: no edits to existing job definitions; the new
   job is purely additive.

**Tests:**
- Workflow validity: run `nix shell nixpkgs#actionlint -c actionlint
  .github/workflows/integration-http.yml` → assert exit 0 (repository
  nix tooling; no undeclared interpreter dependencies).
- Local verification of the command set: each of the four cargo commands
  in step 2 exits 0 in the feature worktree (never the shared main
  checkout; arrange: worktree state after Phase 3 tasks; act: run each
  command; assert: exit 0 and the named test suites appear in output).

**Acceptance:**
- The workflow file parses and references only existing job conventions.
- Local runs exit 0.
- `git diff --stat .github/workflows/ci.yml` is empty.

Spec coverage: integration-tier "Demand-gated activation" — "default suite
untouched", CI isolation sentences.

- [x] 3.4

### Documentation and lint hygiene

#### Task 3.5: CLI full-boot scenario execution, CONTEXT-MAP citation, and lint sweep

**Files:**
- `CONTEXT-MAP.md` (modified: cite ADR-0069 in the testing section)
- `crates/camel-cli/src/commands/test/runner.rs` (modified: full-boot
  scenario execution path)
- `crates/camel-cli/tests/test_scenario_cli_e2e.rs` (new)

**Steps:**
1. Add ADR-0069 to CONTEXT-MAP.md's testing-related ADR citations
   (satisfies `cargo xtask lint-context-citations`).
2. CLI full-boot scenario execution (feature-selected): in
   `test/runner.rs`, for a scenario document whose harness endpoints are
   all of scheme http and the CLI is built with `integration-http`:
   create one `HttpPartner` per harness endpoint (binds `127.0.0.1:0`),
   build the harness-provisioned map from each endpoint's `bind_var`,
   construct the `LayeredEnv` (Task 2.3), call `boot_scenario` (Task
   3.2), execute the actions through `run_scenario(doc, &router,
   &mut vars)` with the caller-owned `PartnerRouter`, then
   `boot.shutdown(&ctx)`; map
   verdict/apparatus failures to exits per the Task 2.5 taxonomy. With
   the feature off, or with non-http scheme endpoints, the existing
   `infra-unavailable` path applies unchanged.
3. End-to-end test: run the actual `camel test` command on the outbound
   fixture document from Task 3.2, with the CLI built `--features
   integration-http` → assert the scenario executes through the full
   boot, the partner listener receives on the wire, the tier annotation
   reports `[full]`, and exit 0.
4. Lint sweep for the two new crates: `cargo xtask
   lint-non-exhaustive` (new public enums `Tier`, `ScenarioAction`,
   `ScenarioFailure`, `DocError`, `ScenarioVerdict` carry
   `#[non_exhaustive]`), `cargo xtask lint-component-deps`,
   `cargo xtask lint-publish-cycles`, `cargo xtask lint-unwrap`,
   `cargo xtask lint-secrets`, `cargo xtask lint-log-levels`,
   `cargo xtask lint-ignore`, `cargo xtask schema --check`.

**Tests:**
- `cli_runs_full_boot_scenario`: arrange the Task 3.2 outbound fixture →
  act: invoke the `camel test` command path with
  `--features integration-http` built in → assert exit 0, `[full]` tier
  annotation in stdout, and the partner's recorded wire request matches
  the sent body.
  Command: `cargo test -p camel-cli --features integration-http --test
  test_scenario_cli_e2e cli_runs_full_boot_scenario`.

**Acceptance:**
- All listed xtask lints exit 0.
- `cargo test -p camel-cli --features integration-http --test
  test_scenario_cli_e2e` passes.

Spec coverage: mock-testkit "full document boots the shared cascade
in-process" (CLI half); documentation-governance conventions.

- [x] 3.5
