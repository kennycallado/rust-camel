# Design: emissionproof

## Approach

Prove the uniform component-operations facade wiring for the two
artifact-gated AUDIT entries by driving the real production paths
in-crate against a shared recording double.

1. **Shared double** — `RecordingRuntimeObservability` in
   `camel-component-api/src/test_support.rs`, beside the existing
   `Noop`/`Panic` doubles. Interior `Mutex<Vec<..>>` records
   `increment_errors(component, label)` and
   `record_component_operation(component, operation, outcome)`; accessors
   return snapshots. `component_metrics()` is overridden to
   `ComponentMetrics::new(self.metrics(), lever)` with the lever set at
   construction, so a single type proves both the lever-gated
   component-ops family and the never-gated error family (failure legs
   run twice: lever on asserts error + op record; lever off asserts the
   error increment survives with zero op records). The `health()`
   side is a no-op registry (health is not under test).

2. **wasm invoke legs** — new
   `crates/components/camel-component-wasm/tests/producer_emission_test.rs`.
   Success: copy the committed echo guest
   (`camel-integration-test/tests/fixtures/wasm/echo.wasm`) into a temp
   base dir, build the endpoint through the real
   `WasmComponent::create_endpoint("wasm:echo.wasm", ..)` (path
   canonicalization exercised), `create_producer` with the recording
   double (lever on), drive `oneshot`, and assert
   `("wasm","invoke","success")` with zero error increments. Failure:
   write a non-module file as the guest, drive the same path, assert
   `increment_errors("wasm","e:wasm:invoke")` plus the `failure` op.

3. **cxf consume legs** — new
   `crates/components/camel-cxf/tests/consumer_emission_test.rs`, reusing
   the existing harness: `spawn_mock_bridge()`,
   `BridgeSlot::new_ready_for_test(channel)`,
   `CxfBridgePool::from_config(..)` + `insert_slot_for_test`, real
   `CxfConsumer::start(ConsumerContext)` with the recording double. A
   handler task answers the context's exchange envelopes: Ok reply
   (success leg), Err reply (route-error leg, run with lever on and
   again with lever off), and a Stream output body (marshalling leg).
   Assertions wait on `MockState` recorded responses, then check
   recorder contents: `("cxf","consume","success")` /
   `("cxf","e:cxf:consume")` + `failure` op / and on the marshalling leg
   the retained route-scoped `("<route-id>","b-prime:cxf:response-marshalling")`
   (the b-prime site passes `ctx.route_id()`, so the test route id is
   chosen distinct from the component name) alongside component-scoped
   `("cxf","e:cxf:consume")` and one `("cxf","consume","failure")` —
   the intended D5 double-count.

4. **Authoritative table** — update only the
   `component_emission_test.rs` module-header block (the "No honest
   harness exists without external artifacts" bullet): wasm and cxf now
   carry crate-internal executable proof; point at the two new test
   files. `AUDIT` and `DRIVABLE` consts are untouched, and
   `audit_table_complete` keeps locking the entry set.

## Affected crates

- `camel-component-api`: +`RecordingRuntimeObservability` under
  `test-support` (no default-feature impact).
- `camel-component-wasm`: new integration test file (guest fixture
  referenced by path from the sister crate; copied to a temp dir at test
  time — no repo duplication).
- `camel-cxf`: new integration test file reusing `tests/support/`
  mock-bridge harness.
- `camel-test`: documentation-only edit inside
  `tests/component_emission_test.rs`.

## Architecture boundaries

Components layer only. No Runtime, DSL, or Services changes; the facade
(`camel-api` `ComponentMetrics`) and both components' production code
are consumers under test, not modification targets. The double lives in
the component API's test support next to its siblings, respecting the
existing test-double seam. Hexagonal boundary: `camel-component-wasm`
and `camel-cxf` already dev-depend on `camel-component-api` with
`features = ["test-support"]`, so no new dependency edges.

## Alternatives considered

- **CI legs gated on the wasm fixture / cxf-bridge binary** (the bd's
  second option): rejected — the native bridge build in CI is heavy and
  nondeterministic, and it would leave the proof invisible to local test
  runs and battery sweeps.
- **Extending camel-test's `DRIVABLE` to wasm/cxf**: rejected — camel-test
  would need guest base-dir plumbing and the bridge binary, which is the
  exact external-artifact problem the module header documents;
  crate-internal tests keep the harness honest.
- **Migrating camel-test's local `EmissionRecorder` to the shared
  double**: deferred — out of scope; no functional gain inside this
  change's risk budget.
