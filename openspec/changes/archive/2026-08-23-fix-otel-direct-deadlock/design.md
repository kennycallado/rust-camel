# Design: fix-otel-direct-deadlock

## Approach

Tower's `Service` contract: a permit/reservation acquired by `poll_ready` must be consumed by `call()` on the SAME instance. `TracingProcessor` violates it — `poll_ready` delegates to the original inner, but `call` clones the inner and re-readies the clone (`tracer.rs:177,191`). For stateful producers (permit stored in `pending_permit`, semaphore Arc shared by `Clone`), the clone's `poll_ready` waits forever on the permit held by the original, whose `call` never runs.

**Phase 1 fix (canonical, camel-core only):** `TracingProcessor` keeps `inner: BoxProcessor` (no `Option` — the wrapper must stay reusable across sequential `poll_ready`/`call` cycles, and its `Clone` impl keeps working). In `call`, first clone, then replace (avoids E0502 double-borrow): `let fresh = self.inner.clone(); let original = std::mem::replace(&mut self.inner, fresh);` — the fresh clone becomes the placeholder, the already-readied ORIGINAL moves into the boxed future — then invoke the original WITHOUT re-readying (the outer executor already drove `TracingProcessor::poll_ready` → the original inner holds any reservation its `call` needs). The swapped-in clone is unreadied, which is correct: the next cycle's `poll_ready` readies it before the next `call`. `poll_ready` delegation unchanged. Span start/end logic unchanged: span starts in `call` before invoking inner, `SpanEndGuard` still ends it.

**Phase 2 hardening (seven components):** move ONLY the semaphore `acquire_owned()` from `poll_ready` into the `call()` async block (before dispatch); delete `pending_permit`/`acquire_fut` fields. Non-semaphore readiness checks stay in `poll_ready`: direct keeps the registry lookup + `fail_if_no_consumers` fail-fast; kafka keeps the stopped-state error; cxf keeps its bridge-state machine (Ready → Ok; Starting/Restarting → Pending with waker; Degraded/Stopped → Err); wasm keeps the init-failed error; jms, opensearch, and grpc have no other check (unconditional `Ready(Ok(()))`; closed-semaphore errors surface inside `call`). Bounded concurrency is preserved: the semaphore still serializes in-flight `call`s; only the acquisition point moves inside the future where the permit lives and dies with one exchange. In the Direct producer the permit is acquired BEFORE the dispatch timeout starts, so permit contention never consumes the round-trip timeout budget. Each producer refactor ships with (a) a readiness test asserting its retained non-semaphore `poll_ready` behavior, (b) an external-permit-hold test proving `call()` pends while all permits are held (acquisition happens inside `call`, before dispatch) — for all seven producers — and (c) for Direct, an additional true two-call test: a consumer that parks its reply keeps call A in flight while call B pends on the semaphore until A completes and releases its permit.

## Affected crates

- `camel-core` (Phase 1): `shared/observability/adapters/tracer.rs` — `TracingProcessor` inner ownership restructure; no public API change (`TracingProcessor::new` signature unchanged).
- `camel-direct` (Phase 2): `DirectProducer` — permit acquire into `call`.
- `camel-kafka` (Phase 2): `KafkaProducer` (`producer.rs:189-206`) — same.
- `camel-jms` (Phase 2): `JmsProducer` (`producer.rs:140-152`) — same.
- `camel-cxf` (Phase 2): `CxfProducer` (`producer.rs:205-213`) — same.
- `camel-component-wasm` (Phase 2): `WasmProducer` (`producer.rs:183-194`) — same.
- `camel-opensearch` (Phase 2): `OpenSearchProducer` (`producer/mod.rs:46-73`) — same.
- `camel-component-grpc` (Phase 2): `GrpcProducer` (`producer/mod.rs:44-92`) — same.
- `camel-test` (Phase 1): new E2E regression test `otel_direct_hop_regression.rs`.
- `camel-otel` (Phase 1, test-only): new span-status test in `tests/integration.rs`; production code untouched.

## Architecture boundaries

Runtime (camel-core) owns the tracing wrapper fix; Components own their producer readiness shape. No DSL, Services, Languages, or Functions changes; camel-otel production code untouched (test-only addition) — the bug is exporter-independent. The fix direction (consume readied original) is the one anticipated by the `SharedSnapshot` invariant comment: poll_ready and call on the same instance.

## Phases

### Phase 1: Fix TracingProcessor tower-contract violation (rc-qoq3)

- **Goal:** eliminate the silent permanent hang for ALL stateful producers with one camel-core change.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** none (private restructure only).
- **Deliverable:** tracer.rs fix + unit tests (shared-permit mock: single cycle AND sequential reuse of the same TracingProcessor instance) + E2E regression test `otel_enabled_direct_hop_completes_and_repeats` in camel-test, using a process-local no-op/in-memory provider — no OTLP network endpoint (the deadlock is exporter-independent; otel config already enables the traced pipeline).
- **Exit-criteria:** new unit + E2E tests pass; existing tracer/direct/otel suites green; `cargo clippy -p camel-core -- -D warnings` clean.

### Phase 2: Readiness hardening for seven producers (rc-y9l3)

- **Goal:** no producer carries a semaphore permit across the poll_ready/call boundary, so a future clone-then-ready wrapper cannot reproduce the deadlock.
- **Dependencies:** Phase 1 landed (tracer tests guard the wrapper; per-producer tests below guard the refactors — Phase 1's direct-only E2E does NOT cover kafka/jms/cxf/wasm/opensearch/grpc, hence these are mandatory here).
- **Externally-visible types/interfaces:** none.
- **Deliverable:** seven producer refactors; each with (a) readiness test for its retained non-semaphore `poll_ready` behavior (direct registry fail-fast; kafka stopped-state error; cxf bridge-state machine incl. Degraded/Stopped errors; wasm init-failed error; jms/opensearch/grpc unconditional Ok) and (b) bounded-concurrency test (second concurrent `call` future pends until the first releases its permit).
- **Exit-criteria:** all seven crates' test suites green; Phase 1 E2E still green; clippy clean on all seven crates.

## Alternatives considered

- **Fresh semaphore per Clone** — rejected: per-clone limits remove aggregate backpressure; clones happen per-exchange.
- **Fix only the seven producers, leave TracingProcessor** — rejected: leaves the tower-contract violation in place; any future stateful producer reintroduces the hang.
- **Timeout/retry in DirectProducer::poll_ready** — rejected: masks the contract violation, adds latency, doesn't fix the other six producers.
