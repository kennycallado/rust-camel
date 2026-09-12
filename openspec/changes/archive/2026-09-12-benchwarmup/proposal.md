# Proposal: benchwarmup

## Why

Protocol A currently compares the first 1,000 warmup messages and stops at that message bound. High-rate HTTP cells reach that bound before JIT and GC behavior settles, so warmup can report `MessageBoundUnconverged` while the runtime is still converging. This contaminates future M2 results and keeps valid cells out of the measured set. This change implements the narrowed trailing-window design for bd `rc-audm.8`.

## What Changes

Change the loadgen warmup driver to collect until the configured wall-clock bound, then evaluate the latest configured sample window. Propagate its new failure reasons through native and fallback benchmark status classification. The message count is a comparison-window size, not a termination bound. Update unit tests and benchmark-harness notes. The sealed `benchmarks/records/20260903T084658Z` record remains untouched; no benchmark numbers are produced or republished.

## Acceptance criteria

- Protocol A collects until the time bound and never terminates early because `max_messages` is reached; it evaluates the trailing `max_messages` samples at termination.
- At the time-bound evaluation, a stable trailing window returns `Stable`; an unstable full window returns `TimeBoundUnconverged`; an incomplete window returns `InsufficientSamples`.
- `MessageBoundUnconverged` remains in the public diagnostic enum for compatibility but is never emitted by Protocol A.
- Unit tests name and assert `late_convergence_uses_trailing_window`, `time_bound_stable_trailing_window`, `time_bound_unconverged_trailing_window`, and `time_bound_insufficient_samples`; `warmup_drive_request_deadline_stops_new_requests` and `warmup_drive_body_deadline_stops_inflight_request` cover request and body-drain deadline enforcement.
- Benchmark notes document the new future-run protocol and preserve the sealed record policy.
- Native and fallback classifiers accept all three warmup reasons, preserve historical evidence, and fail closed on unknown, malformed, missing-status, or conflicting evidence.

## Risk budget

Acceptable risk: warmup may run longer and collect more samples on high-rate cells, bounded by the configured wall-clock timeout and existing caller controls. Out of bounds: changing measurement schemas, altering sealed records, adding adaptive protocol-B behavior, or changing unrelated fixtures.
