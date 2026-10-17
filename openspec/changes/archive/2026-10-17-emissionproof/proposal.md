# Proposal: emissionproof

## Why

bd rc-xlo0 (Task 4.2 review, dashboard-observability): the wasm `invoke`
and cxf `consume` facade wiring has zero executable proof. The `AUDIT`
const in `crates/camel-test/tests/component_emission_test.rs` lists both
entries as authoritative, yet the module header documents them as "no
honest harness exists without external artifacts" — wasm was believed to
need a compiled guest reachable only through endpoint canonicalization,
and cxf the native `cxf-bridge` binary. Recon shows both beliefs are
stale: a committed echo guest exists at
`crates/camel-integration-test/tests/fixtures/wasm/echo.wasm`, and
`camel-cxf` ships a `MockCxfBridge` tonic double plus
`BridgeSlot::new_ready_for_test` that can drive the real `CxfConsumer`
task without the native binary. The facade wiring can therefore be proven
in-crate today; leaving it unproven risks silent regressions of the
dashboard observability contract (ADR-0066 family, design D5
double-count).

## What Changes

- Add `RecordingRuntimeObservability` to
  `camel-component-api::test_support` (feature `test-support`): records
  `increment_errors` and `record_component_operation` calls, exposes the
  `ComponentMetrics` facade with a configurable components lever.
- New `crates/components/camel-component-wasm/tests/producer_emission_test.rs`:
  success leg (committed echo guest, real endpoint+producer) and failure
  leg (non-module file) asserting `e:wasm:invoke` / `wasm:invoke:*`.
- New `crates/components/camel-cxf/tests/consumer_emission_test.rs`:
  success, route-error, and marshalling-failure legs against the mock
  bridge asserting `e:cxf:consume`, `cxf:consume:*`, and the retained
  `b-prime:cxf:response-marshalling` label (D5 double-count).
- Update the `component_emission_test.rs` module-header documentation:
  wasm/cxf move from "no honest harness" to crate-internal proof; the
  `AUDIT` const itself and `DRIVABLE` stay unchanged.

Explicitly excluded: production code paths (no `src/` changes outside
feature-gated test support), CI workflow edits, migrating camel-test's
local `EmissionRecorder` to the shared double, and any new fixture
guests.

## Acceptance criteria

- wasm invoke and cxf consume facade wiring gain executable emission
  proof via crate-internal asserts with a recording
  `RuntimeObservability` double — no native bridge binary, no
  non-committed artifacts.
- The `AUDIT` const in `component_emission_test.rs` remains the
  authoritative table with every entry backed by executable proof.
- All new tests pass under the fleet containment mandate
  (`systemd-run` scope, caps standard).

## Risk budget

Low: test-only change plus one feature-gated test-support type. No
runtime behavior, metrics labels, or public API beyond `test-support`
changes. Out of bounds: touching facade semantics, the AUDIT entry set,
or the cxf bridge process management.

Affected crates: `camel-component-api` (test support),
`camel-component-wasm` (tests), `camel-cxf` (tests), `camel-test` (test
docs).

bd: rc-xlo0
