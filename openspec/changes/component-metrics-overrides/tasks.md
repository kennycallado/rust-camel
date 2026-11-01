# Tasks: component-metrics-overrides

Single-phase change: one coherent slice across config, handle, migration,
HTTP and docs. `design.md` records no `## Phases` section.

Compile-coherence boundary: Tasks 1.1–4.1 keep `cargo test -p <crate>` green
per crate. Task 2.1 intentionally breaks the workspace for every crate that
still calls the old facade; the workspace compiles again only after Task 6.1
(the last migrated caller). Do not run workspace-wide gates before 6.1.

## camel-core: config model

### Task 1.1: Add per-component overrides and the effective-switch accessor

**Files:**
- `crates/camel-core/src/shared/observability/domain/config.rs` (modified)
- `crates/camel-core/src/shared/observability/adapters/tracer_tests.rs` (modified: 3 exhaustive `MetricsLeversConfig` literals at lines ~1160, ~1215, ~1406)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified: exhaustive literal at line ~6466)

**Steps:**
1. Add `pub component_overrides: std::collections::BTreeMap<String, bool>` to
   `MetricsLeversConfig` and to its private `Raw` deserializer struct as
   `#[serde(default)] component_overrides: Option<std::collections::BTreeMap<String, bool>>`
   (absent map -> empty). Keep `#[serde(deny_unknown_fields)]`.
2. Populate it in `Deserialize` with `raw.component_overrides.unwrap_or_default()`.
3. Extend `Default for MetricsLeversConfig` with `component_overrides: BTreeMap::new()`.
4. Add `pub fn component_enabled(&self, component: &str) -> bool` returning
   `self.enabled && self.component_overrides.get(component).copied().unwrap_or(self.components)`.
5. Add `component_overrides: std::collections::BTreeMap::new()` to every
   exhaustive `MetricsLeversConfig { .. }` literal in the two camel-core test
   files listed above, so `cargo test -p camel-core --lib` still compiles.
6. Update the struct rustdoc: precedence formula, master switch applied last,
   error family never gated.

**Tests:** (`cargo test -p camel-core --lib shared::observability::domain::config`)
- `metrics_levers_precedence_matrix`: setup = six literal levers with
  `enabled=true`; action = call `component_enabled("http")`; assert false
  (components=false, absent), false (false,false), true (false,true), true
  (true,absent), false (true,false), true (true,true).
- `metrics_levers_master_switch_beats_override`: setup `enabled=false,
  components=false, overrides={"http":true}`; assert `component_enabled("http") == false`.
- `metrics_levers_defaults_have_no_overrides`: `Default` map empty,
  `component_enabled("http") == false`.
- `metrics_levers_deserializes_component_overrides`: serde_json input
  `{"enabled":true,"components":false,"component_overrides":{"http":true,"kafka":false}}`;
  assert two map entries, `component_enabled("http")==true`,
  `component_enabled("kafka")==false`.
- `metrics_levers_absent_override_inherits_components`: input `{"components":true}`;
  assert `component_enabled("http")==true`.
- `metrics_levers_rejects_unknown_keys`: input `{"component_override":{"http":true}}`;
  assert `Err`.

**Acceptance:**
- `cargo test -p camel-core --lib shared::observability::domain::config` exits 0.
- `cargo test -p camel-core --lib` compiles and exits 0 (literals updated).
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.1

## camel-api: scoped handle

### Task 2.1: Make ComponentMetrics component-scoped and add ComponentMetricsLevers

**Files:**
- `crates/camel-api/src/component_metrics.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)

**Steps:**
1. Change `ComponentMetrics` to hold `collector: Arc<dyn MetricsCollector>`,
   `component: Arc<str>`, `enabled: bool`.
2. Replace `new(collector, components_enabled)` with
   `pub fn new(collector: Arc<dyn MetricsCollector>, component: impl Into<Arc<str>>, enabled: bool) -> Self`.
3. Replace `observe(component, operation, failed)` with
   `pub fn observe(&self, operation: &str, failed: bool)`; forward errors as
   `increment_errors(&self.component, &format!("e:{}:{operation}", self.component))`
   and component operations as `record_component_operation(&self.component, operation, outcome)`.
   Keep the existing `allow-open-label` annotations at the emitting expressions.
4. Add `pub fn component(&self) -> &str` returning the fixed identity.
5. Add `ComponentMetricsLevers { enabled: bool, components: bool,
   overrides: Arc<BTreeMap<String, bool>> }` in the same module, with:
   - `new(enabled: bool, components: bool, overrides: BTreeMap<String, bool>)`
   - `uniform(components: bool)` (true -> enabled=true/components=true;
     false -> enabled=false)
   - `enabled_for(&self, component: &str) -> bool` =
     `self.enabled && self.overrides.get(component).copied().unwrap_or(self.components)`
   - `Default` = all off (empty map).
6. Re-export `ComponentMetricsLevers` from `lib.rs` next to `ComponentMetrics`.
7. Update module rustdoc: identity fixed at construction; error family never
   gated; the snapshot is the effective per-component switch.

**Tests:** (`cargo test -p camel-api --lib component_metrics`)
- `scoped_handle_records_fixed_component`: recording collector, handle for
  `"redis"`, `observe("command", false)` twice -> two
  `("redis","command","success")`, zero errors.
- `lever_off_gates_operations_not_errors`: handle for `"redis"`, `enabled=false`,
  `observe("command", true)` -> exactly one `increment_errors("redis","e:redis:command")`,
  zero ops.
- `lever_on_records_both_outcomes`: `enabled=true`, success then failure ->
  ops success then failure, one error.
- `levers_enabled_for_precedence`: `new(true,false,{"http":true})` -> http true,
  kafka false; `new(false,true,{"http":true})` -> both false.
- `levers_uniform_off`: `uniform(false)` -> `enabled_for("http") == false`.
- `levers_uniform_on`: `uniform(true)` -> `enabled_for("http") == true` and
  `uniform(true).enabled_for("unlisted") == true` (inherits components).
- `composite_collector_receives_once`: `MetricsHandle` with two registered
  recording collectors, handle for `"http"`, `observe("consume", true)` -> each
  collector sees exactly one `("http","consume","failure")` and one
  `e:http:consume` error.
- `late_binding_reaches_collector_registered_after_first_observe`: handle over a
  `MetricsHandle` before registration -> `observe("consume", false)` records
  nothing; register a recording collector; `observe("consume", false)` again ->
  exactly one op reaches it.
- `noop_collector_is_safe`: handle over `Arc::new(NoOpMetrics)`, both outcomes,
  no panic.

**Acceptance:**
- `cargo test -p camel-api --lib component_metrics` exits 0.
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- Intermediate-state note: workspace compilation is intentionally broken after
  this task; it is restored only at Task 6.1. Do not edit crates outside
  `camel-api` here.

- [x] 2.1

## camel-component-api: observability plumbing

### Task 3.1: Thread the per-component lever snapshot through the traits

**Files:**
- `crates/components/camel-component-api/src/component_context.rs` (modified)
- `crates/components/camel-component-api/src/runtime_observability.rs` (modified)
- `crates/components/camel-component-api/src/test_support.rs` (modified)

**Steps:**
1. In `ComponentContext`, replace `component_metrics_enabled(&self) -> bool`
   with `fn component_metric_levers(&self) -> ComponentMetricsLevers
   { ComponentMetricsLevers::default() }`.
2. In `RuntimeObservability`, change the default
   `component_metrics(&self) -> ComponentMetrics` to
   `component_metrics(&self, component: &str) -> ComponentMetrics`; the default
   impl uses `ComponentMetrics::new(self.metrics(), component, false)`.
3. In the blanket `impl<T: ComponentContext> RuntimeObservability for T`,
   implement it as `ComponentMetrics::new(self.metrics(), component,
   <Self as ComponentContext>::component_metric_levers(self).enabled_for(component))`.
4. In `test_support.rs`, make `RecordingRuntimeObservability` hold a
   `ComponentMetricsLevers` built from its existing bool via
   `ComponentMetricsLevers::uniform(bool)`; implement the new
   `component_metrics(&self, component: &str)` with `enabled_for(component)`;
   update its in-module assertions to the two-argument `observe`.
5. Update rustdoc on both traits (scoped identity; errors never gated; default
   all-off).

**Tests:** (`cargo test -p camel-component-api --features test-support`)
- `default_component_metric_levers_are_off`: `NoOpComponentContext.component_metric_levers().enabled_for("http") == false`.
- `recording_double_honors_uniform_lever`: lever on ->
  `component_metrics("wasm").observe("invoke", false)` records one op; lever off
  -> zero ops.
- `blanket_impl_uses_context_levers`: local test context overriding
  `component_metric_levers()` to `new(true,false,{"wasm":true})` ->
  `component_metrics("wasm")` emits, `component_metrics("kafka")` records no op
  but still forwards `e:kafka:consume` on failure.
- `runtime_observability_is_send_sync` still passes.

**Acceptance:**
- `cargo test -p camel-component-api --features test-support` exits 0.
- `cargo clippy -p camel-component-api --all-targets --features test-support -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 3.1

## camel-core: runtime wiring

### Task 4.1: Expose levers from CamelContext, controller and registry contexts

**Files:**
- `crates/camel-core/src/context.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_component_context.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_component_context_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified)
- `crates/camel-core/src/shared/components/domain/registry.rs` (modified)
- `crates/camel-core/src/shared/observability/domain/config.rs` (modified)

**Steps:**
1. Add `MetricsLeversConfig::component_metrics_levers(&self) -> ComponentMetricsLevers`
   building `ComponentMetricsLevers::new(self.enabled, self.components, self.component_overrides.clone())`.
2. In `CamelContext`'s `ComponentContext` impl, replace
   `component_metrics_enabled` with `component_metric_levers` returning
   `self.metrics_levers.component_metrics_levers()`.
3. `ControllerComponentContext`: replace the `component_metrics_enabled: bool`
   field and constructor parameter with
   `component_metric_levers: ComponentMetricsLevers`; the trait method clones it.
4. Update every `ControllerComponentContext::new` construction site in
   `route_controller.rs` to pass `levers.component_metrics_levers()`.
5. `RegistryComponentContext`: store a `ComponentMetricsLevers` built from the
   existing `components_enabled` bool with
   `ComponentMetricsLevers::uniform(components_enabled)`; keep
   `new(registry, metrics, components_enabled)` source-compatible; add
   `pub fn with_component_metric_levers(mut self, levers: ComponentMetricsLevers) -> Self`.
6. Update in-file test doubles/assertions that named the old method.

**Tests:** (`cargo test -p camel-core --lib levers`)
- `context_reports_configured_component_levers`: context + `set_tracer_config`
  with `{enabled:true, components:false, component_overrides:{http:true}}` ->
  `component_metric_levers(&ctx).enabled_for("http") == true`,
  `enabled_for("kafka") == false`.
- `context_master_switch_reports_every_component_off`: same with
  `enabled:false, component_overrides:{http:true}` -> `enabled_for("http") == false`.
- `controller_context_reports_injected_levers`:
  `ControllerComponentContext::new(registry, languages, collector, platform,
  health, Some(route_id), ComponentMetricsLevers::new(true, false,
  BTreeMap::from([("wasm".into(), true)])))` -> wasm true, http false.
- `registry_context_default_and_override`: `RegistryComponentContext::new(registry, None, true)`
  -> `enabled_for("http") == true`; `.with_component_metric_levers(uniform(false))`
  -> `enabled_for("http") == false`; a second context built with
  `new(registry, None, false).with_component_metric_levers(new(true,false,{"wasm":true}))`
  -> wasm true, http false (proves distinct switches per registered component).
- Existing `controller_path_component_metrics_reflects_lever` updated to the
  scoped API and passing.

**Acceptance:**
- `cargo build -p camel-core` exits 0.
- `cargo clippy -p camel-core --lib -- -D warnings` exits 0 (lib target only:
  core test targets pull in the seda/opensearch dev-dependencies, whose facade
  calls are migrated in 5.1/5.2).
- `cargo fmt --check` exits 0.
- Deferred gate: `cargo test -p camel-core --lib levers` and
  `cargo clippy -p camel-core --all-targets` run in Task 5.2's acceptance,
  after the dev-dependency crates are migrated.

- [x] 4.1

## Component call-site migration

### Task 5.1: Migrate the first component group to the scoped handle

**Files:**
- `crates/components/camel-kafka/src/producer.rs` (modified)
- `crates/components/camel-kafka/src/consumer.rs` (modified)
- `crates/components/camel-redis/src/producer.rs` (modified)
- `crates/components/camel-component-seda/src/lib.rs` (modified)
- `crates/components/camel-component-wasm/src/producer.rs` (modified)
- `crates/components/camel-component-surrealdb/src/consumer.rs` (modified)

**Steps:**
1. `camel-kafka/src/producer.rs`: `self.runtime.component_metrics("kafka")` and
   `observe("produce", outcome.is_err())`.
2. `camel-kafka/src/consumer.rs`: `runtime.component_metrics("kafka")` and both
   `observe("consume", send_outcome.is_err())` /
   `observe("consume", step_outcome.is_err())`.
3. `camel-redis/src/producer.rs`: `component_metrics("redis")` and
   `observe("command", command_outcome.is_err())`.
4. `camel-component-seda/src/lib.rs`: both construction sites become
   `component_metrics("seda")`; `observe("consume", result.is_err())` and
   `observe("produce", result.is_err())`.
5. `camel-component-wasm/src/producer.rs`: `component_metrics("wasm")` and
   `observe("invoke", outcome.is_err())`.
6. `camel-component-surrealdb/src/consumer.rs`: `component_metrics("surrealdb")`
   and `observe("query", false)` / `observe("query", true)`.
7. Do not move handle construction across await points; keep the same scope.

**Tests:** (`cargo test -p camel-component-kafka -p camel-component-redis -p camel-component-seda -p camel-component-wasm -p camel-component-surrealdb --lib`)
- Existing facade/emission tests compile and pass after the signature change.
- `scoped_identity_survives_migration` (seda test module): recording runtime,
  switch on for `"seda"`, one producer call -> recorded op carries
  `component="seda"`, `operation="produce"`.

**Acceptance:**
- The test command above exits 0.
- `cargo clippy -p camel-component-kafka -p camel-component-redis -p camel-component-seda -p camel-component-wasm -p camel-component-surrealdb --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 5.1

### Task 5.2: Migrate the remaining component group to the scoped handle

**Files:**
- `crates/components/camel-mail/src/producer.rs` (modified)
- `crates/components/camel-mail/src/consumer.rs` (modified)
- `crates/components/camel-ws/src/client_consumer.rs` (modified)
- `crates/components/camel-ws/src/test_doubles.rs` (modified)
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-opensearch/src/producer/mod.rs` (modified)
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/producer.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (producer site only)

**Steps:**
1. `camel-mail/src/producer.rs`: `rt.component_metrics("mail")`,
   `observe("send", failed)`.
2. `camel-mail/src/consumer.rs`: `component_metrics("mail")`,
   `observe("poll", polled.is_err())`.
3. `camel-ws/src/client_consumer.rs`: both sites become
   `runtime.component_metrics("ws")`, `observe("frame", false)` /
   `observe("frame", true)`.
4. `camel-ws/src/test_doubles.rs`: change the `component_metrics` override to
   `fn component_metrics(&self, component: &str) -> ComponentMetrics` and build
   the handle with its existing lever bool.
5. `camel-cxf/src/consumer.rs`: `component_metrics("cxf")`,
   `observe("consume", response.fault)`.
6. `camel-opensearch/src/producer/mod.rs`: `component_metrics("opensearch")`,
   `observe("execute", result.is_err())`.
7. `camel-rabbitmq/src/consumer.rs`: `component_metrics("rabbitmq")`,
   `observe("consume", outcome == ConsumeOutcome::Failure)`.
8. `camel-rabbitmq/src/producer.rs`: `component_metrics("rabbitmq")`,
   `observe("publish", outcome == PublishOutcome::Failure)`.
9. `camel-http/src/lib.rs` producer site: `self.runtime.component_metrics("http")`,
   `observe("request", outcome.is_err())`.
10. Do NOT touch `camel-jms`'s `memory.observe(observation)` — different type
    (health observation), not the metrics facade.

**Tests:** (`cargo test -p camel-component-mail -p camel-component-ws -p camel-component-cxf -p camel-component-opensearch -p camel-component-rabbitmq --lib`)
- Existing per-crate facade/emission tests compile and pass (including the cxf
  recording-double proofs).
- `producer_http_request_unchanged` (camel-http lib tests): a producer call
  failure records `("http","request","failure")` and `e:http:request`, and no
  `consume` op.

**Acceptance:**
- The test command above exits 0.
- `cargo test -p camel-component-http --lib producer` exits 0.
- Deferred gate from Task 4.1: `cargo test -p camel-core --lib` exits 0 (full
  core lib suite, so every promised core scenario executes, not only the
  `levers`-filtered ones) and `cargo clippy -p camel-core --all-targets -- -D warnings`
  exits 0 (the core test targets now build against the migrated
  seda/opensearch dev-dependencies).
- `cargo clippy -p camel-component-mail -p camel-component-ws -p camel-component-cxf -p camel-component-opensearch -p camel-component-rabbitmq -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- Workspace compilation is still broken (camel-config, camel-cli, redis-repo,
  prometheus tests); it is restored at Task 6.1.

- [x] 5.2

## camel-config + services + CLI

EXECUTION NOTE: during Task 5.1 the plan-ordering fix migrated the redis-repo facade call sites and the `context_ext.rs` `component_enabled("redis")` wiring early (compile blockers). Task 6.1 keeps validation, the CLI builder, camel-test signatures and prometheus tests.

### Task 6.1: Validate overrides, wire the redis facade, and restore the workspace

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/context_ext.rs` (modified)
- `crates/camel-config/src/config_tests/component_overrides_tests.rs` (new)
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-test/tests/redis_tls_test.rs` (modified: two `ComponentMetrics::new` call sites)
- `crates/camel-test/tests/redis_sentinel_tls_test.rs` (modified)
- `crates/camel-test/tests/redis_repositories_test.rs` (modified)
- `crates/camel-test/tests/cache_payload_offload.rs` (modified)
- `crates/services/camel-redis-repo/src/executor.rs` (modified)
- `crates/services/camel-redis-repo/src/idempotent_repo.rs` (modified)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified)
- `crates/services/camel-redis-repo/src/payload_store.rs` (modified)
- `crates/services/camel-redis-repo/src/test_metrics.rs` (modified)
- `crates/services/camel-prometheus/src/metrics/tests.rs` (modified)

**Steps:**
1. Add `fn is_component_identity(key: &str) -> bool` in `config.rs`: reject
   empty; first byte must be ASCII lowercase letter or digit; remaining bytes
   ASCII lowercase letter, digit, `_` or `-`.
2. In `CamelConfig::validate()`, iterate
   `self.observability.metrics.component_overrides.keys()` and return
   `CamelError::Config(format!("observability.metrics.component_overrides.{key}: invalid component identity"))`
   for the first invalid key.
3. In `context_ext.rs`, change `redis_repo_component_metrics` to
   `ComponentMetrics::new(metrics, "redis", config.observability.metrics.component_enabled("redis"))`,
   and update the exhaustive `MetricsLeversConfig` literal at ~line 2678 with
   `component_overrides: BTreeMap::new()`.
4. In `camel-cli/src/commands/run.rs`, chain
   `.with_component_metric_levers(cfg.observability.metrics.component_metrics_levers())`
   on the `RegistryComponentContext` construction.
5. Update all redis-repo constructors/tests to
   `ComponentMetrics::new(collector, "redis", lever)` and two-argument `observe`.
6. Update `camel-prometheus/src/metrics/tests.rs` to the scoped API
   (`ComponentMetrics::new(collector, "redis", lever)`); keep both existing
   render assertions (`prometheus_renders_family`,
   `components_lever_suppresses_family_render`).
7. Register the new module in `crates/camel-config/src/config.rs` beside the
   existing sibling modules (`#[path = "config_tests/component_overrides_tests.rs"]
   mod component_overrides_tests;`). `config_tests/` has no `mod.rs`; do not add one.
8. Update the four camel-test files listed above to
   `ComponentMetrics::new(collector, "redis", false)` and two-argument `observe`.

**Tests:** (`cargo test -p camel-config --lib component_overrides`)
- `component_overrides_reject_empty_key`: fixture TOML
  `[observability.metrics.component_overrides]\n"" = true` written to a
  tempfile, `CamelConfig::from_file` + `validate()` -> `Err` whose message
  contains `observability.metrics.component_overrides`.
- `component_overrides_reject_invalid_identity`: keys `HTTP` and `http.v2` in
  separate fixtures -> `Err` naming the key.
- `component_overrides_accept_valid_identities`: keys `http`, `kafka`,
  `camel-https`, `a_b-9` -> `validate()` Ok.
- `component_overrides_profile_merges_key_by_key`: fixture with
  `[default.observability.metrics.component_overrides] http = true, kafka = true`
  and `[prod.observability.metrics.component_overrides] http = false`, loaded
  with profile `prod` -> map `http=false`, `kafka=true`.
- `component_overrides_include_contributes`: base file
  `include = ["extra.toml"]` (array form; the string form is rejected by the
  parser), `extra.toml` defines
  `[observability.metrics.component_overrides] kafka = true` -> merged map
  contains `kafka=true`.
- `component_overrides_unregistered_component_is_inert`: fixture overrides
  `not_a_component = true` -> `validate()` Ok; the resolved levers report
  `component_enabled("not_a_component") == true` while `component_enabled("http")`
  keeps the global default, proving no registered component inherits it.
- `default_exposition_excludes_component_family`: with default levers (no
  table), a scoped `ComponentMetrics` for `"http"` observes a success and the
  prometheus `gather()` body does NOT contain `camel_component_operations_total`
  (extends `components_lever_suppresses_family_render` to the default table).
- `redis_facade_resolves_per_component_switch`: levers
  `components=false, component_overrides.redis=true` -> the redis facade records
  an op; without the override it records none but still forwards
  `e:redis:add`/`e:redis:command` on failure.
- redis-repo and prometheus suites pass under the scoped signature.

**Acceptance:**
- `cargo test -p camel-config --lib component_overrides` exits 0.
- `cargo test -p camel-redis-repo --lib` and `cargo test -p camel-prometheus --lib` exit 0.
- `cargo test -p camel-test --features integration-tests --tests --no-run` exits 0 (compile-only).
- `cargo build --workspace` exits 0 (workspace compilation restored here).
- `cargo clippy -p camel-config -p camel-cli -p camel-redis-repo -p camel-prometheus --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 6.1

## camel-test vocabulary

### Task 7.1: Extend the emission vocabulary

**Files:**
- `crates/camel-test/tests/component_emission_test.rs` (modified)
- `crates/camel-test/tests/metrics_wiring_test.rs` (modified if it calls the facade)

**Steps:**
1. Change the http entry in `COMPONENT_OP_SETS` from `&["request"]` to
   `&["request", "consume"]`. The table is a static vocabulary asserted for
   membership only (not derived from live emission), so this is safe before
   9.1/9.2; run tasks in ID order.
2. Update module rustdoc: http carries both the producer (`request`) and
   consumer (`consume`) operations; the consumer leg is proven in the
   component crate.
3. If `metrics_wiring_test.rs` constructs the facade, migrate it to
   `ComponentMetrics::new(collector, "http", lever)` and two-argument `observe`.

**Tests:** (`cargo test -p camel-test --test component_emission_test`)
- `http_roundtrip_observed` still passes unchanged (producer labels).
- `audit_vocabulary_covers_http_consume`: the `COMPONENT_OP_SETS` http entry
  contains both `request` and `consume`.
- The non-Docker legs of the suite pass.

**Acceptance:**
- `cargo test -p camel-test --test component_emission_test` exits 0.
- `cargo fmt --check` exits 0.

- [x] 7.1

## camel-http: completion meter

### Task 8.1: Add the exactly-once consume outcome meter

**Files:**
- `crates/components/camel-http/src/consume_meter.rs` (new)
- `crates/components/camel-http/src/lib.rs` (modified: `mod consume_meter;`)
- `crates/components/camel-http/Cargo.toml` (modified: add `http-body.workspace = true`)
- `Cargo.toml` (modified: add `http-body = "1"` under `[workspace.dependencies]`)

**Steps:**
1. Define `ConsumeOutcome` as an `Arc<AtomicU8>` with states
   `Unrecorded=0, Succeeded=1, Failed=2, Delegated=3` and this transition table
   (every transition is `compare_exchange`, first writer wins, terminal states
   never change):
   - guard drop: `Unrecorded -> Failed`;
   - dispatch failure before delegation: `Unrecorded -> Failed`;
   - dispatch handoff: `Unrecorded -> Delegated`;
   - body completion: `Delegated -> Succeeded`;
   - body error / drop before completion: `Delegated -> Failed`.
   Methods: `record_failure_before_delegation(&self, m: &ComponentMetrics)`,
   `record_success_after_delegation(&self, m: &ComponentMetrics)`,
   `record_failure_after_delegation(&self, m: &ComponentMetrics)`,
   `delegate(&self) -> bool`, `is_final(&self) -> bool`.
2. Define `ConsumeGuard { outcome: ConsumeOutcome, metrics: ComponentMetrics }`
   whose `Drop` runs the guard-drop transition.
3. Define `MeterBody<B> { inner: B, outcome: ConsumeOutcome, metrics: ComponentMetrics }`
   implementing `http_body::Body` (`Data = B::Data`, `Error = B::Error`):
   - `poll_frame`: `Some(Ok(frame))` -> return it, then if `inner.is_end_stream()`
     run the completion transition; `Some(Err(_))` -> failure transition; `None`
     -> completion transition.
   - `is_end_stream` / `size_hint` delegate; `Drop` of an armed meter runs the
     failure transition.
   - `pub fn inner_is_end_stream(&self) -> bool`.
4. Delegation owns the transition: the dispatch layer calls `outcome.delegate()`
   before handing the wrapped body to Axum, so the guard's Drop can never fire
   on the normal path.
5. Unit tests poll manually with `futures::task::noop_waker`; no new dev-dep.

**Tests:** (`cargo test -p camel-component-http --lib consume_meter`)
- `guard_drop_records_failure_once`: guard, drop -> one failure + one error;
  a second transition attempt records nothing.
- `delegated_body_success_on_exhaustion`: delegate, one-frame body, poll to
  `None` -> one success, no error; dropping afterwards records nothing.
- `delegated_body_drop_before_exhaustion_is_failure`: delegate, non-empty body,
  drop unpolled -> one failure.
- `final_frame_end_stream_is_success_without_another_poll`: delegate, body whose
  only frame sets end-of-stream, poll that frame then drop -> one success, no
  failure.
- `body_error_is_failure_once`: delegate, error frame, poll, then drop -> one
  failure, no success.
- `empty_body_records_success_when_flagged`: delegate, `inner_is_end_stream()`
  true -> the dispatch-side completion transition runs once.
- `success_after_delegation_is_not_doubled_by_drop`: delegate, completion
  transition, drop the meter -> no failure.
- `undelegated_body_cannot_record_success`: no `delegate()`, run the completion
  transition -> nothing recorded (state still `Unrecorded`).

**Acceptance:**
- `cargo test -p camel-component-http --lib consume_meter` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 8.1

## camel-http: dispatch wiring

### Task 9.1: Carry per-route metrics and emit consume at response completion

**Files:**
- `crates/components/camel-http/src/registry.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/benches/registry.rs` (modified if it destructures the registry)

**Steps:**
1. Add `#[doc(hidden)] pub struct RouteEntry { pub sender:
   mpsc::Sender<RequestEnvelope>, pub metrics: camel_api::ComponentMetrics }`
   deriving `Clone`, in `registry.rs`; change `api_routes` to
   `HashMap<String, RouteEntry>` and `rest_endpoints` to
   `Vec<RestEndpoint<RouteEntry>>`; update register/unregister/probe code,
   `ProbeLookup`, `Debug`, and the benchmark probe.
2. In `HttpConsumer::start`, build
   `let route_metrics = self.runtime.component_metrics("http")` and register it
   with the sender; keep registration/cleanup semantics identical.
3. Add `failed: bool` to `HttpReply`; set `true` for the cancel-503,
   auth-denial, stream-already-consumed, and pipeline-error replies, `false` for
   the success path. Update every construction site.
4. In `dispatch_handler`: select `(sender, metrics)` from the same entry; create
   a `ConsumeGuard` once an entry is selected. Record failure before delegation
   for: `reply.failed`, send error, reply-channel error, the 413 rejection, the
   inflight 503 rejection, the oversized-response replacement, and a response
   whose headers cannot build a valid `Response` (the existing
   `unwrap_or_else` fallback).
5. On a valid reply: if the request method is `HEAD`, or the reply status is
   `204`/`304`/`1xx`, delegate then record success. Otherwise wrap the body in
   `MeterBody`, delegate, and if `inner_is_end_stream()` record success.
6. Verify no URL/path/header/body value reaches the facade and that the producer
   site is untouched.

**Tests:** (`cargo test -p camel-component-http --lib`)
- The existing suite compiles and passes (no regressions).
- `cargo test -p camel-component-http --lib --no-run` exits 0.
- Scenario coverage is proven in 9.2.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 9.1

### Task 9.2: Prove the consume scenarios end to end

**Files:**
- `crates/components/camel-http/src/lib_tests.rs` (modified)

**Steps:**
1. Reuse the existing `HttpConsumer::start` harness patterns in the file plus
   `camel_component_api::test_support::RecordingRuntimeObservability`.
2. Each test below is a full contract: setup, action, exact assertion.
3. For streaming cases, build the reply body from a `futures::stream` in a test
   route and read (or drop) the response body explicitly.

**Tests:** (`cargo test -p camel-component-http --lib consume`)
- `consume_success_recorded_on_body_completion`: setup = consumer route
  answering `ok`, recorder lever on; action = GET and read the full body; assert
  exactly one `("http","consume","success")`, zero errors.
- `consume_failure_on_pipeline_error`: setup = route returning `Err`; action =
  GET and read the body; assert one `("http","consume","failure")` and one
  `e:http:consume`, and no success after the error body is exhausted.
- `consume_route_selected_error_status_is_success`: setup = route setting
  `CamelHttpResponseCode = 503` and completing; action = GET and read the body;
  assert one success, zero errors.
- `consume_empty_get_is_success`: setup = route returning an empty body; action
  = GET and read the body; assert one success, zero failures.
- `consume_head_and_204_and_304_are_success`: setup = three routes (HEAD
  request, 204 reply, 304 reply); action = issue each request and read
  available bytes; assert one success per request, zero failures.
- `consume_streaming_reply_completes`: setup = route returning a two-frame
  stream; action = read the body to end; assert one success.
- `consume_streaming_reply_aborted_early`: setup = route returning a stream
  whose first frame is gated on a oneshot that the test never releases; action
  = drive `dispatch_handler` directly, take the returned response body, poll
  zero frames, then drop it; assert one failure and one `e:http:consume`, no
  success. Do not rely on a client-side drop of a fully buffered body.
- `consume_stream_error_is_failure_once`: setup = route returning a stream whose
  second frame is an error; action = read to the error then drop; assert one
  failure, no success.
- `consume_oversized_response_is_failure`: setup = `maxResponseBody` smaller
  than the route reply; action = GET and read the replacement body; assert one
  failure + `e:http:consume`, no success.
- `consume_payload_too_large_is_failure`: setup = `maxRequestBody` smaller than
  the sent body; action = POST; assert one failure + `e:http:consume` without
  the route observing an exchange.
- `consume_inflight_exhaustion_is_failure`: setup = `maxInflightRequests = 1`
  and a slow route; action = two concurrent GETs; assert the rejected request
  records one failure + `e:http:consume`.
- `consume_closed_channel_is_failure`: setup = `AppState` whose registry holds
  an api route sender with no receiver, driven through `dispatch_handler`;
  action = one request; assert one failure + `e:http:consume`.
- `consume_dispatch_cancelled_while_awaiting_is_failure`: setup = a route whose
  receiver never replies, `AppState` from that registry; action = call
  `dispatch_handler`, poll the returned future once (so it is parked in
  `sender.send(..)` or `reply_rx.await`), then drop the future without polling
  again; assert exactly one `("http","consume","failure")` and one
  `e:http:consume`, and no later success.
- `consume_shared_listener_uses_route_metrics`: setup = two consumers on one
  `host:port`, distinct recording runtimes, one switch on and one off; action =
  one successful request to the switched-on route and one FAILING request to
  the switched-off route; assert the switched-on recorder sees exactly its own
  success op and no error, the switched-off recorder sees exactly one
  `e:http:consume` and zero ops, and neither recorder sees the other route's
  emission.
- `consume_switched_off_still_forwards_errors`: setup = lever off + pipeline
  error; action = GET; assert `e:http:consume` recorded, zero ops.
- `producer_request_labels_unchanged`: setup = producer pointing at a failing
  upstream; action = producer call; assert `("http","request","failure")` +
  `e:http:request`, zero consume ops.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 9.2

## Docs, ADR, schema, context

### Task 10.1: Document the override switch, the consume metric and the scoped handle

**Files:**
- `docs/adr/0066-metrics-collector-binding-and-lifetime.md` (modified)
- `docs/src/services/metrics.md` (modified)
- `docs/src/configuration/schema.md` (modified)
- `crates/camel-api/CONTEXT.md` (modified: `ComponentMetrics` entry)
- `crates/camel-core/CONTEXT.md` (modified: `RuntimeObservability` entry)
- `crates/components/camel-component-api/CONTEXT.md` (modified: `test-support` entry)

**Steps:**
1. ADR-0066: amend Decision 5 with the per-component effective switch
   (`enabled && override.unwrap_or(components)`; errors still ungated); amend
   Decision 14 vocabulary with `http` = `request` (producer) and `consume`
   (consumer); add Decision 16 "per-component override switch" recording the
   precedence, the identity-validation rule, and that a scoped handle fixes the
   identity.
2. `docs/src/services/metrics.md`: add `http:consume` to the operation list,
   the completion/abort semantics (body exhaustion = success, body error /
   prior drop = failure, route-selected status irrelevant), and a
   `component_overrides` example.
3. `docs/src/configuration/schema.md`: add the `component_overrides` row (type
   `table`, empty default, precedence formula, identity validation, recursive
   profile/include merge).
4. Update the three `CONTEXT.md` glossary entries.
5. Schema regeneration note: the metrics config has no generated schema artifact
   (`cargo xtask schema` covers canonical route, DSL, component-metadata and TS
   only). `crates/camel-config/src/config.rs` is the executable schema, and the
   validation tests in Task 6.1 are the machine check; `schema.md` is the doc.
   Record this in the ADR decision so "schema regenerated" is explicitly
   discharged as N/A for generated files and covered by validate + docs.

**Tests:**
- `cargo xtask schema --check` exits 0 (no generated drift; the config schema is
  not among the generated artifacts).
- `cd docs && mdbook test` exits 0.

**Acceptance:**
- `cargo xtask schema --check` exits 0.
- `cd docs && mdbook test` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-component-api -p camel-component-http --no-deps` exits 0.
- `openspec validate component-metrics-overrides --type change --strict` exits 0.

- [x] 10.1
