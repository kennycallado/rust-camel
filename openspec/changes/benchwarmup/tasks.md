# Tasks: benchwarmup

## Phase 1: Correct Protocol A warmup convergence

### benchmarks/harness/loadgen

### Task 1.1: Implement trailing-window Protocol A warmup

**Spec coverage:** `Protocol A warmup uses a trailing stability window`, scenarios `Late convergence replaces early drift`, `Stable trailing window at time bound`, `Unstable trailing window at time bound`, and `Insufficient trailing samples at time bound`; `Warmup enforces its wall-clock deadline`, scenario `In-flight request cannot extend warmup indefinitely`.

**Files:**
- `benchmarks/harness/loadgen/src/warmup.rs` (modified)
- `benchmarks/harness/loadgen/src/cli_runtime.rs` (modified)

**Steps:**
1. Update `check_warmup_stability` to select the latest `cfg.max_messages` samples and split that window into equal halves; preserve `WarmupConfig`, `WarmupOutcome`, and diagnostic enum names.
2. Return `Stable` or `TimeBoundUnconverged` only from the complete trailing window at the wall-clock evaluation; return `InsufficientSamples` when the deadline arrives before one complete comparison window; never emit `MessageBoundUnconverged` from Protocol A.
3. Change `warmup_drive` to collect until its wall-clock deadline instead of iterating only to `cfg.max_messages`.
4. Apply deadline guards before request start and around both response receipt and body drain; discard in-flight work that cannot finish before the deadline and do not start another request after deadline.
5. Rewrite existing `fails_when_second_half_drifts_beyond_10pct` and `time_bound_unconverged_after_full_sample_window` tests to assert trailing-window and `TimeBoundUnconverged` behavior instead of `MessageBoundUnconverged`; update comments and diagnostics without changing result schema.

**Tests:** (executable spec — name, arrange, act, assert)
- `late_convergence_uses_trailing_window`: arrange a 2,000-sample input whose first 1,000 samples drift and latest 1,000 samples agree; call `check_warmup_stability` at the configured deadline; assert `Stable` and p50 fields equal latest-window halves.
- `time_bound_stable_trailing_window`: arrange one complete stable trailing window; call at elapsed time beyond the configured limit; assert `Stable { elapsed_ns }` preserves actual elapsed time.
- `time_bound_unconverged_trailing_window`: arrange one complete unstable trailing window; call at the deadline; assert `FailedStability { reason: TimeBoundUnconverged, .. }`.
- `time_bound_insufficient_samples`: arrange fewer than `max_messages` samples; call at the deadline; assert `FailedStability { reason: InsufficientSamples, .. }`.
- `warmup_drive_request_deadline_stops_new_requests`: arrange a test server with a request counter and a deterministic paused response, use a 50ms warmup deadline, call `warmup_drive`, capture counter value N at return, wait 100ms, re-read the counter, and assert it remains N and returned sample count excludes blocked work.
- `warmup_drive_body_deadline_stops_inflight_request`: arrange a test server that sends headers then blocks body drain, use a 50ms warmup deadline, call `warmup_drive`, and assert completion occurs within 250ms of start and the blocked sample is discarded.
- Command: `cargo test --manifest-path benchmarks/harness/loadgen/Cargo.toml`.
- Expected before implementation: trailing-window and driver deadline tests fail because current code truncates at the first `max_messages` samples and has no in-flight deadline.

**Acceptance:**
- `cargo test --manifest-path benchmarks/harness/loadgen/Cargo.toml` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo clippy --manifest-path benchmarks/harness/loadgen/Cargo.toml --all-targets -- -D warnings` exits 0.
- Protocol A no longer terminates at `max_messages`; the pure criterion uses only the latest complete window at time-bound evaluation.
- Request and body-drain deadline tests prove warmup cannot extend beyond its configured wall-clock bound.

- [x] 1.1

## Phase 2: Record future-run benchmark policy

### benchmarks documentation

### Task 2.1: Document and propagate future-run warmup status

**Spec coverage:** `Warmup documentation states the future-run protocol`, scenario `Operator reads warmup policy`.
Also covers `Fail-closed complete-record publish`, scenario `trailing-window warmup failure counts as present` while preserving historical `MessageBoundUnconverged`, status-line validation, conflict handling, timeout evidence, and missing-cell fail-closed behavior.
The task also owns retained scenarios `complete record publishes clean`, `pre-reference records stay complete`, `missing metric rejects publish`, `wholly missing cell rejects publish`, `n/a warm is not a gap`, `unconverged warmup counts as present with status`, `probe timeout counts as present with status`, `measured wins over attempt evidence`, `conflicting statuses stay missing`, `malformed evidence stays missing`, and `status schema is additive and one-way`.

**Files:**
- `benchmarks/harness/CONTEXT.md` (modified)
- `benchmarks/harness/test_warmup_policy.py` (new)
- `benchmarks/docs-investigation-strategy.md` (modified)
- `benchmarks/harness/run.sh` (modified)
- `benchmarks/harness/summarize.py` (modified)
- `benchmarks/harness/test_summarize.py` (modified)

**Steps:**
1. Add the Protocol A warmup decision to the benchmark methodology section, stating that `max_messages` is a trailing comparison-window size and the wall-clock bound controls termination.
2. State that `MessageBoundUnconverged` is retained only for compatibility, Protocol B is unchanged, and the sealed 20260903 record is never modified or republished.
3. Add `test_warmup_policy.py` with `test_protocol_a_policy_documented`, which reads `CONTEXT.md`, asserts the trailing-window, wall-clock, Protocol-B exclusion, and sealed-record phrases, walks exactly the three sealed files, compares their SHA-256 values against pinned constants `CAVEATS.md=b4cb35aab10f5b23bc7720062ced5e98b0029cb80e25a9bf173ecb5941e08044`, `run.json=fe14eca6e69c55a5d5bd725772731e0b92f8e3bbcddd8924d1b8bd1b08342b84`, and `summary.md=b824f0f184010b0e7c7e3d68d832574fb6d4cad872845316289bfdc9ea77b71e`, and fails on extra or missing files.
4. Link the live defect disposition in `benchmarks/docs-investigation-strategy.md` section 8 without copying sealed measurements.
5. Accept `MessageBoundUnconverged`, `TimeBoundUnconverged`, and `InsufficientSamples` in both native shell status emission and Python fallback classification, while retaining the required measure-a error status line and fail-closed behavior for unknown or conflicting evidence.
6. Add summary tests covering all three reasons and future cells with no latency fields.

**Tests:** (executable spec — name, arrange, act, assert)
- `test_protocol_a_policy_documented`: arrange the modified `CONTEXT.md` and repository tree; read the notes and sealed-record paths; assert each policy term appears in the warmup methodology section and no sealed record file is changed.
- Command: `python3 -m unittest discover -s benchmarks/harness -p 'test_*.py'`.
- Expected before implementation: documentation search test fails because the notes lack any Protocol A warmup policy section.

**Acceptance:**
- `python3 -m unittest discover -s benchmarks/harness -p 'test_*.py'` exits 0 and runs `test_protocol_a_policy_documented`.
- The same command runs classifier tests for all three warmup reasons and historical fail-closed cases.
- `test_native_m2_attempt_status_helper`: arrange synthetic round evidence for each of the three reason strings; act through the native status helper; assert status `unconverged` for each reason; expected before implementation: new reasons fail.
- `test_native_warmup_reason_statuses`: arrange the harness `run.sh`; read its native failed-stability grep regex; assert it names `MessageBoundUnconverged`, `TimeBoundUnconverged`, and `InsufficientSamples`; expected before implementation: the regex names only the historical reason or is absent.
- `test_classify_m2_accepts_all_warmup_failure_reasons`: arrange each reason with the required failure status line, plus unknown and missing-status evidence; act through `classify_m2_attempt`; assert attempted status for all three reasons and missing for invalid evidence.
- `test_m2_future_reason_cells_emit_status`: arrange future-reason round evidence and conflicting timeout evidence; act through record construction; assert attempted cells have no latency fields and conflicting evidence remains missing; expected before implementation: new reasons fail.
- `bash -n benchmarks/harness/run.sh` exits 0.
- `CONTEXT.md` documents trailing-window termination, deadline enforcement, Protocol-B exclusion, and sealed-record policy in English.
- Native and fallback classifiers accept all three reasons and reject unknown, incomplete, or conflicting evidence.
- No file under `benchmarks/records/20260903T084658Z/` is modified.

- [x] 2.1
