# Tasks: emissionproof

Operational note (fleet mandate, bd rc-xlo0 mission 333): every cargo
command below runs under the containment wrapper —
`systemd-run --user --scope --collect --unit=fleet-emissionproof -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache <cargo command> -j4`
— and the scope is stopped (`systemctl --user stop fleet-emissionproof.scope`)
after each run. Commands are written un-wrapped below for readability.

## camel-component-api

### Task 1.1: RecordingRuntimeObservability test double

**Files:**
- `crates/components/camel-component-api/src/test_support.rs` (modified)

**Steps:**
1. In `test_support.rs`, add a private `struct RecorderState { errors: Mutex<Vec<(String, String)>>, ops: Mutex<Vec<(String, String, String)>> }` (std `Mutex`, like the recording double in `crates/camel-api/src/component_metrics.rs` unit tests) and a private `#[derive(Clone)] struct RecorderCollector { state: Arc<RecorderState> }`.
2. `impl MetricsCollector for RecorderCollector`: `increment_errors(&self, component: &str, error_type: &str)` pushes `(component.to_string(), error_type.to_string())`; `record_component_operation(&self, component: &str, operation: &str, outcome: &str)` pushes the 3-tuple; every other trait method is a no-op (mirror the full method list of the trait in `crates/camel-api/src/metrics.rs`, including `increment_retry_attempt` if present on the trait — check the trait, do not guess).
3. Add `pub struct RecordingRuntimeObservability { state: Arc<RecorderState>, components_enabled: bool }` with `pub fn new(components_enabled: bool) -> Arc<Self>`, `pub fn errors(&self) -> Vec<(String, String)>` and `pub fn ops(&self) -> Vec<(String, String, String)>` (clone-and-return snapshots).
4. `impl HealthCheckRegistry for RecordingRuntimeObservability` — no-op `force_unhealthy_for_route` (mirror `NoopRuntimeObservability`).
5. `impl RuntimeObservability for RecordingRuntimeObservability`: `metrics()` returns `Arc::new(RecorderCollector { state: Arc::clone(&self.state) })`; `health()` returns `Arc::new(NoopRuntimeObservability)`; override `component_metrics()` to `ComponentMetrics::new(self.metrics(), self.components_enabled)` (import path mirrors `runtime_observability.rs` in the same crate).
6. Doc-comment the type: recording double for emission proofs; lever baked in at construction so one type proves both the never-gated error family and the lever-gated component-ops family.
7. Add a `#[cfg(test)] mod recording_tests` inside `test_support.rs` with the two unit tests below.

**Tests:** (executable spec)
- `recording_double_captures_facade_emissions`: setup `RecordingRuntimeObservability::new(true)` → act `rt.component_metrics().observe("wasm", "invoke", false)` then `.observe("wasm", "invoke", true)` → assert `ops() == [("wasm".into(), "invoke".into(), "success".into()), ("wasm".into(), "invoke".into(), "failure".into())]` and `errors() == [("wasm".into(), "e:wasm:invoke".into())]`.
- `lever_off_suppresses_ops_not_errors`: setup `new(false)` → act `.observe("cxf", "consume", true)` → assert `errors() == [("cxf".into(), "e:cxf:consume".into())]` and `ops().is_empty()`.
- command: `cargo test -p camel-component-api --lib recording_double` and `cargo test -p camel-component-api --lib lever_off` — expected: pass after step 7 (fail before: compile error, module absent).

**Acceptance:**
- `cargo test -p camel-component-api --lib` passes (pre-existing suite + 2 new).
- `cargo clippy -p camel-component-api -- -D warnings` exits 0.
- `cargo fmt --check` clean on the touched file.

- [x] 1.1

## camel-component-wasm

### Task 2.1: wasm invoke emission legs

**Files:**
- `crates/components/camel-component-wasm/tests/producer_emission_test.rs` (new)

**Steps:**
1. Create the test file with imports mirroring `tests/integration.rs`: `camel_api::{Body-not-needed, CamelError, Exchange, Message, ProducerContext}`, `camel_component_api::{Component, ComponentContext, Endpoint, NoOpComponentContext, RuntimeObservability}`, `camel_component_wasm::WasmComponent`, `tempfile::tempdir`, `tower::ServiceExt`, `std::{fs, path::PathBuf, sync::Arc}`.
2. Helper `fn guest_src() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../camel-integration-test/tests/fixtures/wasm/echo.wasm") }`.
3. Helper `async fn drive(base: &std::path::Path, guest_name: &str, rt: Arc<dyn RuntimeObservability>) -> Result<Exchange, CamelError>`: `WasmComponent::new(Arc::new(NoOpComponentContext), base.to_path_buf())` → `create_endpoint(&format!("wasm:{guest_name}"), &NoOpComponentContext)` (expect Ok) → `create_producer(rt, &ProducerContext::new())` (expect Ok) → `producer.clone().oneshot(Exchange::new(Message::new("hello-wasm"))).await`. The `"hello-wasm"` text body is the exact payload shape the echo guest processes end-to-end in `crates/camel-integration-test/tests/wasm_boot_test.rs`.
4. Write the three tests below; each creates its own tempdir, and the success leg copies `guest_src()` into the tempdir as `echo.wasm` before driving; the failure legs write `not-module.wasm` with bytes `b"this-is-not-a-wasm-component"` (same content as `integration.rs`'s invalid-module test).

**Tests:** (executable spec)
- `wasm_invoke_success_emits_component_operation`: setup tempdir + copied echo guest, `RecordingRuntimeObservability::new(true)` → act `drive(base, "echo.wasm", rt.clone())` → assert result `Ok`, `rt.ops() == [("wasm","invoke","success")]`, `rt.errors().is_empty()`.
- `wasm_invoke_failure_emits_error_family_and_failure_op`: setup tempdir + invalid file, `new(true)` → act `drive(base, "not-module.wasm", rt)` → assert result is `Err` matching `CamelError::Config(_)` containing "wasm compilation failed", `rt.errors()` contains `("wasm","e:wasm:invoke")`, `rt.ops()` contains `("wasm","invoke","failure")`.
- `wasm_invoke_failure_with_lever_off_still_emits_error_family`: same as above with `new(false)` → assert errors contains `("wasm","e:wasm:invoke")` and `rt.ops().is_empty()`.
- command: `cargo test -p camel-component-wasm --test producer_emission_test` — expected: pass after this task (before: target does not exist).

**Acceptance:**
- All 3 tests pass under the wrapper.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0; `cargo fmt --check` clean.
- No changes to any `src/` file of the crate.

- [x] 2.1

## camel-cxf

### Task 3.1: cxf consume emission legs

**Files:**
- `crates/components/camel-cxf/tests/consumer_emission_test.rs` (new)

**Steps:**
1. `mod support;` and reuse `support::mock_bridge::{spawn_mock_bridge, MockState}` plus the wait-helpers pattern from `tests/consumer_unit_test.rs` (`wait_for_consumer_request_sender`, `wait_for_recorded_responses` — copy them locally; they are private to that file). Both helpers call `acquire_deadline`, so import it explicitly: `use camel_component_api::test_support::acquire_deadline;`.
2. Helper `async fn start_consumer(rt: Arc<dyn RuntimeObservability>, route_id: &str) -> Result<(MockState, CancellationToken, mpsc::Receiver<ExchangeEnvelope>, CxfConsumer), Box<dyn std::error::Error + Send + Sync>>`: spawn mock bridge → connect `Channel` → `BridgeSlot::new_ready_for_test(channel)` → `CxfBridgePool::from_config(CxfPoolConfig { profiles: vec![], ..Default::default() })` + `insert_slot_for_test(CxfBridgePool::slot_key(), slot)` → `CxfConsumer::new(Arc::new(pool), "emission-proof".into(), rt)` → `(tx, rx) = mpsc::channel(16)` → `ConsumerContext::new(tx, token.clone(), route_id.to_string())` → `consumer.start(ctx).await?` → `Ok((state, token, rx, consumer))` (all fallible steps propagate with `?`). Behavior selection lives in each test's handler (step 3), NOT in this helper. Use route id `"emission-proof-route"` in every leg (distinct from the component name `"cxf"`).
3. Per-leg handler task: each test calls `start_consumer(...)` and IMMEDIATELY `tokio::spawn`s the handler consuming the returned `rx`, binding the `JoinHandle` in a local that lives until the test's assertions complete. Each test declares which `Behavior` (`enum Behavior { ReplyOk, ReplyErr, ReplyStreamBody }`, defined in the test file) its handler applies: `ReplyOk` → set output body `Body::Text("<ok/>".into())` and `reply_tx.send(Ok(exchange))`; `ReplyErr` → `reply_tx.send(Err(CamelError::ProcessorError("route rejected".into())))`; `ReplyStreamBody` → set output body to `Body::Stream(StreamBody)` built exactly like the doc example at `crates/camel-api/src/body.rs` lines 57-60 — `stream: Arc::new(Mutex::new(Some(Box::pin(<zero-item stream>))))`, `metadata: Default::default()` — where the pinned stream is `futures::stream::empty()` typed to the item type `StreamBody`'s stream field requires (read the `StreamBody` definition; the stream must yield zero items so the body is unmarshallable without hanging) — then `reply_tx.send(Ok(exchange))`. The handler must be running BEFORE the test sends its ConsumerRequest, or the consumer's `send_and_wait` waits forever. `reply_tx` is `Option` — `.expect` on it is NOT allowed (lint-unwrap): match or `if let`.
4. Each test: `start_consumer(...)` → spawn handler (step 3) → `wait_for_consumer_request_sender(&state)` → send `Ok(ConsumerRequest { request_id: "req-em-1", operation: "op", payload: b"<in/>", headers: Default::default(), soap_action: "urn:op", security_profile: String::new() })` → `wait_for_recorded_responses(&state, 1)` → assert the recorded response's `fault` flag → assert recorder contents → cleanup: `token.cancel()`, await `consumer.background_task_handle()` with a 2s timeout (mirror `consumer_task_exits_on_context_token_cancel`).

**Tests:** (executable spec)
- `cxf_consume_success_emits_operation`: setup lever-on double, `ReplyOk` → act as step 4 → assert recorded response `fault == false`, `rt.ops() == [("cxf","consume","success")]`, `rt.errors().is_empty()`.
- `cxf_consume_route_error_emits_error_family`: setup lever-on, `ReplyErr` → assert `fault == true`, `rt.errors()` contains `("cxf","e:cxf:consume")`, `rt.ops()` contains `("cxf","consume","failure")`.
- `cxf_consume_failure_with_lever_off_still_emits_error_family`: setup lever-off, `ReplyErr` → assert errors contains `("cxf","e:cxf:consume")` and `rt.ops().is_empty()`.
- `cxf_marshalling_failure_retains_b_prime_and_double_counts`: setup lever-on, route id `"emission-proof-route"`, `ReplyStreamBody` → assert `fault == true`; EXACT-COUNT assertions: `rt.errors()` contains EXACTLY ONE `("emission-proof-route","b-prime:cxf:response-marshalling")` and EXACTLY ONE `("cxf","e:cxf:consume")` (count matches, not just contains), and `rt.ops() == [("cxf","consume","failure")]` (exactly one op — the D5 double-count lives on the error family only).
- command: `cargo test -p camel-component-cxf --test consumer_emission_test` — expected: pass after this task (before: target does not exist).

**Acceptance:**
- All 4 tests pass under the wrapper, no `#[ignore]`, no sleeps beyond the existing wait-helpers' polling.
- `cargo clippy -p camel-component-cxf -- -D warnings` exits 0; `cargo fmt --check` clean.
- No changes to any `src/` file of the crate; `tests/support/` untouched.

- [x] 3.1

## camel-test

### Task 4.1: AUDIT header documentation points at the executable proofs

**Files:**
- `crates/camel-test/tests/component_emission_test.rs` (modified, documentation-only)

**Steps:**
1. In the module-header doc comment, replace the final bullet ("**No honest harness exists without external artifacts** (documented, not faked): wasm — … cxf — … Both stay in `AUDIT` and are verified by their crates' integration suites plus CI.") with a bullet to the effect of: "**Proven crate-internally (mission 333, bd rc-xlo0)**: wasm `invoke` and cxf `consume` carry executable emission proof in their own crates — `crates/components/camel-component-wasm/tests/producer_emission_test.rs` (real endpoint+producer over the committed echo guest) and `crates/components/camel-cxf/tests/consumer_emission_test.rs` (real consumer task over the in-crate mock bridge, no native binary) — both using the shared `RecordingRuntimeObservability` from `camel-component-api` test support. They stay in `AUDIT` as non-`DRIVABLE` here because camel-test cannot host their harnesses; every `AUDIT` entry carries executable failure-family proof, and wasm and cxf now carry full success+failure legs in-crate."
2. Read `audit_table_complete` (and `dead_components_now_emit`) to confirm no code change is needed — the `AUDIT` const and `DRIVABLE` const stay byte-identical.
3. Re-read the edited doc block once for STE-compliance: short sentences, no AI filler.

**Tests:** (executable spec)
- `audit_table_complete` (existing, unchanged): suite still passes with the `AUDIT` const listing exactly the six swept pairs → command `cargo test -p camel-test --test component_emission_test` (default features) — expected: pass, zero source-line changes outside the doc comment (`git diff` shows comment lines only).

**Acceptance:**
- `git diff` on the file touches only `//!` doc lines.
- `cargo test -p camel-test --test component_emission_test` passes under the wrapper.
- No stale claim remains that no honest harness exists for wasm/cxf.

- [x] 4.1
