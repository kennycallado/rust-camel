# Proposal: shared-log-capture-harness

## Why

The inline tracing log-capture test harness has five consumers, past the
promotion trigger (precedent: `NoopRuntimeObservability`, bd rc-421es).
Every copy drifts independently, and the tracing-subscriber guard rules
(`ensure_global_tracing_default`, rc-img5 / rc-6jarb,
`docs/testing/tracing-capture-guards.md`) are subtle: one shared
implementation is safer than five.

## What Changes

- New shared module `camel_component_api::test_support::log_capture`
  (public test-support surface only, gated by the existing `test-support`
  Cargo feature; new opt-in `traced-test-capture` feature for the
  `tracing-test` global-buffer family):
  - `CaptureWriter` + sync `capture_logs` + async `capture_logs_async`
    (from camel-core site 3 and camel-component-wasm site 4 — identical
    writer/format contract, two entry points);
  - `capture_warns` (camel-config site 2, moved verbatim);
  - `capture_debugs_with_span_records`, `captured_field`, `record_field`
    (camel-processor site 1, moved verbatim);
  - `traced::{lock_log_capture, clear_captured_logs, captured_records_containing}`
    (camel-platform-kubernetes site 5 — the `#[traced_test]` global-buffer
    family, a deliberately different capture mechanism).
- All five inline copies become thin local wrappers that keep calling
  their own colocated, process-global `ensure_global_tracing_default` /
  `ensure_global_registry` guard (hard constraint: guard bodies are
  one-per-test-binary and never move; rc-img5 / rc-6jarb).
- `camel-cli` was checked for log-capture consumers: none exist, so no
  camel-cli change is included (lane 192 coordination satisfied).
- `skip_specs: true` (mission order): test tooling only, no normative
  behavior; no capability spec is modified or invented.

## Acceptance criteria

- Shared surface exists in `camel-component-api`, feature-gated,
  doc-commented (lint-missing-docs-ratchet clean).
- All five sites import the shared module; consumer test behavior is
  byte-identical: no assertion loosened, no test deleted or renamed.
- Guard bodies stay colocated one-per-test-binary at each consumer.
- No production-code change; no new tracing features; no camel-cli change.
- Mission gates pass: targeted `-p` test batteries, `cargo build
  --workspace`, `cargo fmt --check --all`, clippy with AGENTS.md excludes
  `-D warnings`, full `cargo xtask` lint battery, containment via
  `systemd-run --user --scope`.

## Risk budget

- Acceptable: verbatim moves only; the camel-config `capture_warns`
  level predicate (`>= Level::WARN`) is kept byte-identical even though
  tracing-core's reversed `Level` Ord makes it capture everything except
  `error!` (documented honestly in the shared docs; fix deferred to a
  follow-up bd issue, not this change).
- Out of bounds: touching guard bodies' placement/timing/`OnceLock`
  semantics; converting site 5 to a thread-local capture (it must stay a
  global recording buffer — the leadership loop logs from spawned tasks);
  inventing a capability spec.
