# Design: benchwarmup

## Approach

Keep `WarmupConfig` and public outcome types stable. Make `max_messages` a trailing comparison-window size, not a termination cap. The warmup driver MUST collect until the wall-clock bound, then call the pure criterion once. At that point, compare the first and second halves of the latest window. Return `Stable` when trailing halves are within tolerance; otherwise return `TimeBoundUnconverged`. If the time bound arrives before a full window, return `InsufficientSamples`. `MessageBoundUnconverged` remains for public compatibility but is not emitted by Protocol A.

Adjust the warmup driver so it does not treat the message window as terminal. Enforce the wall-clock deadline before starting each request and around each in-flight request/body drain with a timeout; discard a request that cannot finish before the deadline. Preserve the existing diagnostic enum and output schema. Add deterministic tests: `late_convergence_uses_trailing_window` arranges an unstable first window and stable latest window and asserts `Stable` p50 values; `time_bound_stable_trailing_window` asserts `Stable` at the actual elapsed time; `time_bound_unconverged_trailing_window` asserts `TimeBoundUnconverged`; `time_bound_insufficient_samples` asserts `InsufficientSamples`; `warmup_drive_request_deadline_stops_new_requests` arranges a blocked response, acts through `warmup_drive`, and asserts no post-deadline request is started; and `warmup_drive_body_deadline_stops_inflight_request` arranges a response whose body drain blocks, acts through `warmup_drive`, and asserts the elapsed warmup is bounded and the sample is discarded.

## Affected crates

- `benchmarks/harness/loadgen`: trailing-window criterion, caller behavior, and unit tests.
- `benchmarks/harness/run.sh`, `summarize.py`, and tests: preserve attempted-status classification for new warmup failure reasons.

## Architecture boundaries

This is benchmark harness control-plane logic. It does not change runtime, component, DSL, service, language, or function behavior. It changes only when the harness permits Protocol A measurement and keeps the existing result shape.

## Phases

### Phase 1: Correct Protocol A warmup convergence
- **Goal:** Collect through the wall-clock bound and evaluate the latest complete trailing window with deadline-safe request handling.
- **Dependencies:** Existing loadgen warmup criterion and reqwest client.
- **Externally-visible types/interfaces:** No new public types; existing warmup outcomes remain schema-stable.
- **Deliverable:** Updated loadgen implementation and deterministic Rust tests.
- **Exit-criteria:** Loadgen tests, formatting, and clippy pass; no Protocol A path terminates at `max_messages`.

### Phase 2: Record future-run benchmark policy
- **Goal:** Document the trailing-window protocol, sealed-record boundary, and downstream attempted-status classifier contract.
- **Dependencies:** Phase 1 behavior, existing benchmark methodology notes, and record summarizer/publisher contract.
- **Externally-visible types/interfaces:** No code interface; operator-facing harness documentation.
- **Deliverable:** Updated benchmark context, investigation strategy link, classifier implementation/tests, and policy test.
- **Exit-criteria:** Targeted Python harness tests pass, shell syntax passes, future and historical failure reasons classify as attempted, and sealed-record hashes remain unchanged.

## Alternatives considered

Keeping the first-window comparison was rejected because it reproduces the live defect. A separate adaptive-extension mechanism was rejected because it belongs to Protocol B and would create an unbounded or schema-changing warmup policy. Changing the configured timeout was rejected because it does not fix first-window selection at high message rates.
