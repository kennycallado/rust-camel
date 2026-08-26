# Design: trace-model-tree

Technical design for the trace model fix. Ruling: `docs/rulings/trace-model-tree-ruling-2026-08-26.md` (untracked, local). This file carries the durable decisions.

## Current Flow (defect)

`compose_traced_pipeline` (route_compiler.rs) wraps each step in `TracingProcessor`. Per step: `parent_cx = exchange.otel_context`, start step span, then `exchange.otel_context = cx_with_step_span` — and it stays that way. The next step chains under the previous step span. No span encloses the route. `TracedPipeline::call` (route_compiler.rs:331) currently adds no span.

## New Flow

### Route root span — `TracedPipeline`

At `TracedPipeline::call` entry:

1. `entry_cx = exchange.otel_context.clone()`.
2. Start span `{route_id}` (kind `Internal`), parent `entry_cx`. Attributes: `messaging.system=camel`, `route_id`, `correlation_id` (capped). If `entry_cx` has no valid span, the SDK starts a new trace — correct for consumer entry.
3. `exchange.otel_context = entry_cx.with_span(root_span)` — derive from `entry_cx`, never from `OtelContext::current()`, so baggage and typed context values survive.
4. Run the existing step sequence unchanged.
5. Restore `exchange.otel_context = entry_cx` when the outcome carries an exchange (`Ok`/`Completed`/`Stopped`); a `Failed` outcome maps to a tower `Err` with no exchange — the root span closes with the error status and the error propagates (the caller keeps its own tree; direct callers nest, aggregators continue in caller context).
6. On completion set root status (`Ok`/`error(e.to_string())`) and record the `exception` event on error. Root ends via `SpanEndGuard` drop (panic-safe), same pattern as steps.

Steps become siblings under the root; containment holds because the root opens before step 0 and ends after the last step.

**Empty traced routes.** `compose_traced_pipeline*` currently returns `IdentityProcessor` for an empty step list before `TracedPipeline` is built. With tracing enabled the empty pipeline SHALL still be a `TracedPipeline` with zero steps, so the root span opens and closes uniformly. The untraced path keeps `IdentityProcessor`.

### Structural segments — spans in `run_steps`

Splitters and multicast compile to `CompiledStep::Segment` (ADR-0025) and are dispatched by `run_steps`, not wrapped by `TracingProcessor` — today they emit no span. `run_steps` already receives the `trace` flag from `TracedPipeline::call`. When `trace` is set, each `Segment` step SHALL get the same span treatment as a `Process` step, inline in `run_steps`:

1. `entry_cx = ex.otel_context.clone()` (the route root).
2. Start span `{route_id}:step-{i}` (kind `Internal`) with the standard attributes, parent `entry_cx`.
3. `ex.otel_context = entry_cx.with_span(segment_span)` before the segment executes — fragment exchanges clone this live context (`splitter.rs:309`), so fragment-driven sub-route roots nest under the segment span.
4. Restore `ex.otel_context = entry_cx` after the segment completes (`Completed`/`Stopped` outcomes, which carry the exchange); end the span with result status and the `exception` event on failure (`PipelineOutcome::Failed` carries no exchange — span closes, error propagates).

**Retries.** The initial segment invocation goes through `run_steps`, but the error handler retries segments by calling `RetryableStep::invoke` directly (`route_compiler.rs:420-445`). A span-aware adapter (`TracedSegmentStep` implementing `RetryableStep`) wraps the segment for BOTH the initial invocation and `handler.retry_step` — every attempt (initial and retries, wherever invoked) runs the same triple: each attempt derives its span context from the exchange's current context (the route root), runs, and ends its own span with that attempt's result. Restoration to the root context happens on outcomes that carry an exchange (`Completed`/`Stopped`); a `Failed` attempt has no exchange to restore — its span closes with the `exception` event. Retry fragments then nest under the retry attempt's span, never under the route root. camel-api's error handler is unchanged.

The context bookkeeping is shared with `TracingProcessor` via a small helper (start/restore/end triple) to avoid duplicating the attribute construction.

### Step nesting — `TracingProcessor`

Change only the context bookkeeping around `inner.call`:

- Keep: `parent_cx = exchange.otel_context` (now the root), start step span, `exchange.otel_context = parent_cx.with_span(step_span)` for the duration of `inner.call` — derived from `parent_cx`, not from `OtelContext::current()`, so baggage survives. Split fragments (`splitter.rs` clones the live context) and `direct:` producers invoked inside the step nest under the step span.
- Fix: after `inner.call` returns `Ok`, restore `exchange.otel_context = parent_cx` (the root). Today the step span leaks to the next step — this single change turns the comb into a fan. On `Err` there is no exchange to restore; the step span closes with the `exception` event.
- Result exchange from `inner.call` may carry a modified `otel_context` (sub-route root restored its entry context there); the restore overwrites it deliberately so the next step stays a root child.

### Splitter

`fragment_exchange` (camel-api splitter.rs:309) unchanged — fragments clone the live splitter-step context. After the fix that is the correct parent. Doc comment updated to state the nesting contract. New-trace-per-item threshold is bd rc-29gd.

### Exception event (P1-b)

In both `TracingProcessor` error branch and `TracedPipeline` root error path: `add_event("exception", [KeyValue::new("exception.type", class), KeyValue::new("exception.message", msg)])`. Frozen string literals; no `opentelemetry-semantic-conventions` dependency (avoids a new dep edge in camel-core). Replace the existing `"error"` event. `set_status(Status::error(...))` stays.

### duration_ms (P1-a)

Delete the `cx.span().set_attribute("duration_ms", …)` line in `TracingProcessor`. Keep the tracing-log field (`tracing::Span::current().record("duration_ms", …)`) and `metrics.record_exchange_duration` — metrics stay per-route histograms, unaffected.

### Parent-based sampling — camel-otel

`to_sdk_sampler` (service.rs:308) wraps every configured root in `Sampler::ParentBased`: `AlwaysOn` → `ParentBased(AlwaysOn)`, `AlwaysOff` → `ParentBased(AlwaysOff)`, `TraceIdRatioBased(r)` → `ParentBased(TraceIdRatioBased(r))`. Exact variant shape verified against the vendored opentelemetry-sdk 0.32.x at implementation time. `OtelSampler` public enum gains no variant — wrapping is internal. Consequence: an exchange arriving with an unsampled parent produces no spans (correct: partial traces disappear instead of lying).

## Affected Crates / Boundaries

- **camel-core** (`route_compiler.rs`, `shared/observability/adapters/tracer.rs`): root span, restore fix, exception event, attr removal. Runtime boundary; no DSL change.
- **camel-api** (`splitter.rs`): doc contract only.
- **services/camel-otel** (`service.rs`): sampler wrapping.
- **camel-test** (`otel_direct_hop_regression.rs`, integration tests): shape assertions updated.

No component changes. `Exchange.otel_context` field type unchanged.

## Tests

- tracer.rs unit tests: root restore per step; step parent = root; `duration_ms` attr absent; `exception` event on error (step level).
- route_compiler tests: `TracedPipeline` opens/ends root; nested entry (injected live context) roots as child; entry context restored on exit; empty traced route still opens a root span; segment step opens `{route_id}:step-{i}` span, restores root after, records `exception` on `PipelineOutcome::Failed` (root-level failure event and error status asserted too); segment retry via error handler opens a span per attempt and retry fragments nest under the retry attempt's span.
- splitter tests: fragment parent = segment span context (extends `test_fragment_exchange_inherits_otel_context`).
- camel-otel tests: `to_sdk_sampler` returns ParentBased for each variant (shape), plus behavioral `ShouldSample` tests — valid sampled remote parent records children, unsampled parent records none, deterministic without exporter or network.
- camel-test: direct-hop regression — sub-route spans nest under caller step; whole-route containment assertion with `InMemorySpanExporter`.

## Risks

- Step clones in `poll_ready` cycles: restore must happen on the result exchange inside the async body, not on `self` state (TracingProcessor clones per cycle — pattern already established).
- Hot-reload pipeline swap: root span lives in `call`, not in stored state — unaffected by swap.
- Deferred P2 items interact later (SpanKindHint reads the same span builders; name enrichment touches `span_name` format guarded by tracer.rs:490 test).
