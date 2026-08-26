# trace-model Specification

## Purpose
TBD - created by archiving change trace-model-tree. Update Purpose after archive.
## Requirements
### Requirement: Route execution opens a route root span

Every traced route execution SHALL open a root span named `{route_id}` at pipeline entry, before the first step starts.

- When the incoming exchange carries no valid span context, the root span starts a new trace.
- When the incoming exchange carries a valid span context (for example a `direct:` sub-route invocation), the root span SHALL be a child of that context.
- The root span SHALL end after the last step completes, with a status reflecting the pipeline result.
- The exchange context SHALL be restored to the entry context when the pipeline returns an exchange (success outcomes).

#### Scenario: Fresh route entry starts a new trace

- **Given** a route `effisNorm` with tracing enabled and an exchange whose `otel_context` has no valid span
- **When** the pipeline processes the exchange
- **Then** a span named `effisNorm` exists with no parent, and every step span of the execution is its descendant

#### Scenario: Nested entry via direct sub-route

- **Given** a `direct:sub` route invoked from a step of a caller route with a live active span
- **When** the sub-route pipeline processes the exchange
- **Then** the sub-route root span is a child of the caller's step span, in the same trace

#### Scenario: Entry context restored on pipeline exit

- **Given** an exchange entering a pipeline with a valid entry context
- **When** the pipeline completes
- **Then** the returned exchange carries the entry context, not the route root span

#### Scenario: Empty traced route still opens a root span

- **Given** a traced route compiled with zero steps
- **When** the pipeline processes an exchange
- **Then** a root span named `{route_id}` is recorded with a success status, and the entry context is restored

### Requirement: Structural segments emit step spans

Each `CompiledStep::Segment` step (split, streaming split, multicast) SHALL emit a step span with the same shape as a process step span, opened by `run_steps` when tracing is enabled.

- The segment span SHALL be a child of the route root span.
- The exchange context SHALL carry the segment span for the duration of the segment execution, and be restored to the route root context afterwards on outcomes that carry an exchange (`Completed`/`Stopped`). This applies to every segment attempt, including error-handler retries invoked through `RetryableStep::invoke`.
- On `PipelineOutcome::Failed`, the segment span SHALL set error status and record the `exception` event.

#### Scenario: Splitter segment nests fragment processing under its span

- **Given** a traced route whose split step compiles as a segment that produces fragments driving sub-routes
- **When** the segment runs
- **Then** a step span exists for the segment as a child of the route root, and each fragment's sub-route root span is a child of the segment step span

#### Scenario: Segment restores root context after completion

- **Given** a traced route with a segment step followed by another step
- **When** the segment completes and the next step starts
- **Then** the next step's parent is the route root span

#### Scenario: Retried segment attempt keeps fragment containment

- **Given** a traced segment that fails on its first attempt and the route error handler retries it through `RetryableStep::invoke`
- **When** the retry attempt runs fragment sub-routes
- **Then** each retry fragment's sub-route root span is a child of the retry attempt's segment span, never of the route root

### Requirement: Step spans nest under the route root

Each step span SHALL be a child of the route root span, not of the previous step span.

- The step span SHALL be the active context for the duration of its inner processor call, so split fragments and synchronous sub-route invocations nest under the step.
- After the inner processor call returns an exchange (success outcomes), the exchange context SHALL be restored to the route root context before the next step runs. An error return carries no exchange; the step span closes with the error status.

#### Scenario: Steps are siblings under the root

- **Given** a traced route with steps 0..2
- **When** the pipeline processes an exchange
- **Then** all three step spans have the route root span as parent, and each starts within the root span's time window

#### Scenario: Step context restored for the next step

- **Given** two consecutive traced steps in a route
- **When** step 0 completes and step 1 starts
- **Then** step 1's parent is the route root span, and step 1 starts after step 0 ends but before the root ends

### Requirement: Splitter fragments nest under the splitter step

Fragment exchanges created by a split SHALL inherit the live splitter step span context, so fragment processing spans are children of the splitter step span within one trace.

#### Scenario: Split fan-out stays in one trace

- **Given** a route whose splitter step produces N fragment exchanges, each driving a traced sub-route
- **When** all fragments complete
- **Then** every sub-route root span is a child of the splitter step span, and all spans share the triggering execution's trace

### Requirement: Sampler is parent-based

The configured sampler SHALL be wrapped in `Sampler::ParentBased`, so child spans respect the parent sampling decision.

#### Scenario: Unsampled parent records no child spans

- **Given** a sampler built with a ratio-based root and an exchange arriving with a valid but unsampled parent context
- **When** `should_sample` is evaluated for a child span of that parent
- **Then** the decision is `Drop`, deterministically, without any exporter or network

#### Scenario: Sampled parent keeps children

- **Given** an exchange arriving with a sampled parent context
- **When** `should_sample` is evaluated for a child span of that parent
- **Then** the decision is `RecordAndSample` regardless of the configured root ratio

#### Scenario: All configured sampler variants wrap parent-based

- **Given** each `OtelSampler` variant (`AlwaysOn`, `AlwaysOff`, `TraceIdRatioBased`)
- **When** `to_sdk_sampler` converts it
- **Then** the result is a parent-based sampler delegating to that root

### Requirement: Failed spans record the exception event

A span whose processing fails SHALL record an event named `exception` with attributes `exception.type` and `exception.message`, in addition to setting the error status. The previous non-standard `error` event SHALL NOT be emitted.

#### Scenario: Error step emits exception event

- **Given** a traced step whose inner processor returns an error
- **When** the step span completes
- **Then** the span carries exactly one event named `exception` with a non-empty `exception.type` and `exception.message`, and its status is error

#### Scenario: Failed pipeline root records exception

- **Given** a traced route whose pipeline fails
- **When** the route root span completes
- **Then** the root span status is error and it records an event named `exception` with `exception.type` and `exception.message`

### Requirement: No duration attribute on spans

Spans SHALL NOT carry a `duration_ms` attribute; duration is derived from span timestamps.

#### Scenario: Step span omits duration_ms

- **Given** a traced step that completes
- **When** the exported span is inspected
- **Then** no attribute named `duration_ms` exists, while the `camel_tracer` log field `duration_ms` is still recorded

