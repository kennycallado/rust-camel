# Proposal: fix-otel-direct-deadlock

## Why

With `observability.otel.enabled = true`, any exchange crossing a `to: "direct:<x>"` InOut step hangs forever: no response, no error, no log, no timeout. Latent since at least v0.31.0 (verified on 0.31/0.32/0.33); only surfaced now because camel-cache introduced `direct:` seams while running OTel. Field mitigation is `otel.enabled = false`, losing all traces.

Root cause (verified by expert consultation, file:line evidence): `TracingProcessor::call` (`camel-core/src/shared/observability/adapters/tracer.rs:177,191`) clones its inner processor and calls `.ready()` on the CLONE, while `TracingProcessor::poll_ready` readied the ORIGINAL. `DirectProducer` (`camel-direct/src/lib.rs`) acquires its sole `Semaphore::new(1)` permit in `poll_ready` into `pending_permit`; `Clone` shares the semaphore Arc but drops the permit. The clone blocks forever on `acquire_owned()`. The direct producer's 30s timeout lives inside `call()` — never reached. Same latent deadlock in Kafka, JMS, CXF, WASM, OpenSearch, and gRPC producers (identical stateful-permit pattern). SEDA/LLM are unaffected (`poll_ready` returns `Ready(Ok(()))`).

Bd: rc-qoq3 (P0 bug), rc-y9l3 (hardening task, discovered-from rc-qoq3).

## What Changes

- **Phase 1 (rc-qoq3, the fix):** `TracingProcessor::call` consumes the already-readied ORIGINAL inner service via `mem::replace` (fresh clone swapped in as placeholder) instead of clone+re-ready — the wrapper stays reusable (`Service` semantics preserved: subsequent `poll_ready`/`call` cycles on the same instance work). One change fixes all seven affected producers. Unit test with a shared-permit mock inner (including sequential-cycle reuse); E2E regression test (two sequential InOut hops through `direct:` with tracing enabled, no OTLP endpoint needed — in-memory/no-op provider).
- **Phase 2 (rc-y9l3, defense-in-depth):** move ONLY the semaphore permit acquisition from `poll_ready` into `call()`'s future for the seven stateful producers (Direct, Kafka, JMS, CXF, WASM, OpenSearch, gRPC). Each producer keeps its non-semaphore readiness checks in `poll_ready` (direct: registry fail-fast; kafka: stopped-state error; cxf: bridge-state Ready/Pending/Err behavior; wasm: init-failed error; jms/opensearch/grpc: none — unconditional `Ready(Ok(()))`). Per-producer readiness and bounded-concurrency tests accompany each refactor.

Excluded: no exporter changes, no OTLP protocol changes, no new public API, no SEDA/LLM modifications.

## Acceptance criteria

- With OTel enabled, a route with `to: "direct:x"` InOut completes, and a SECOND exchange also completes (the original bug wedges the sole permit after the first).
- Unit test: `TracingProcessor` wrapping a mock inner whose `Clone` shares a `Semaphore::new(1)` permit-across-poll boundary completes within timeout.
- All seven producers keep their existing behavior tests green (direct registry fail-fast unchanged; kafka/jms/cxf/wasm/opensearch/grpc suites unchanged).
- Existing tracer/direct/otel test suites show no regression.

## Risk budget

Acceptable: internal restructuring of `TracingProcessor` ownership (private field); producer `poll_ready` simplification (internal). Out of bounds: public API changes, changes to SEDA/LLM, exporter/OTLP behavior, blocking calls in async contexts, loosening any quality gate.
