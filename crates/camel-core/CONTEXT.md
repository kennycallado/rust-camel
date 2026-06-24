# Runtime

The execution engine for rust-camel. Owns Exchange lifecycle, Route management, and the registries that wire together Components, Languages, Functions, and Services.

## Language

**Exchange**:
The unit of data flowing through a Route — an envelope carrying input/output Messages, properties,
extensions, exchange pattern, error state, and optional correlation/tracing context.
`exchange.input` is always present; `exchange.output` is typically set only for `InOut` exchanges.
_Avoid_: message (when referring to the whole Exchange), event, request

**Message**:
The body+headers container inside an Exchange. `exchange.input` is the incoming Message carrying
the payload from the Consumer; `exchange.output` is an optional response Message produced by a
Processor for `InOut` exchanges. Components construct the initial Message; Processors read and
write it through Exchange accessors.
_Avoid_: exchange, event, request, envelope

**ExchangePattern**:
Declares whether an Exchange expects a reply. `InOnly` = fire-and-forget; `InOut` = request-reply.
_Avoid_: MEP (use the enum variant names directly)

**Route**:
A named message-processing pipeline: a source endpoint that emits Exchanges and a sequence of steps that transform or route them.
_Avoid_: flow, pipeline (when referring to the Route as a whole)

**Suspended Route**:
Route lifecycle state where Consumer intake is stopped while the Pipeline and route channel remain alive. `resume` recreates the Consumer without rebuilding the whole Route.
_Avoid_: stopped route, paused pipeline

**Starting Route**:
Route lifecycle state where start intent has been accepted and projected, but the runtime Consumer/Pipeline side effect has not yet been confirmed. Operators can observe this state in Route status queries.
_Avoid_: transient start flag, hidden startup state

**Pipeline**:
The compiled assembly of Processors that processes an Exchange through a Route's data plane.
_Avoid_: chain, middleware stack

**Processor**:
A single processing unit in a Pipeline — either an EIP pattern (filter, choice, split, setBody) or any custom step that receives and returns an Exchange.
_Avoid_: handler, middleware, transformer

**CamelContext**:
The composition root of the runtime. Manages component, language, function, and service registries; controls Route lifecycle.
_Avoid_: container, application context, context (unqualified)

**RuntimeObservability**:
Narrow trait implemented by the runtime and injected into Component Endpoints at `create_consumer` / `create_producer` time. Provides `metrics()` (counter increments for categories b′/e) and `health()` (forced-unhealthy signalling for category g) so Component code can record failures without taking a hard dependency on metrics/health infrastructure. Established by ADR-0012 Phase A.
_Avoid_: metrics handle, observability service, runtime hook (use RuntimeObservability when describing the trait)

**UnitOfWorkConfig**:
Optional per-Route configuration that wraps the Pipeline with exchange lifecycle tracking. When present, fires completion or failure hooks (by producer URI) when an Exchange exits the Pipeline.
_Avoid_: transaction, scope, UnitOfWork

**RuntimeBus**:
CQRS façade for the Route control plane. Commands mutate Route state (start, stop, add); queries read current status from projections.
_Avoid_: event bus, message bus, command bus (unqualified)

**Route Lifecycle Projection**:
Read-side Route status record maintained from lifecycle aggregate state. It is the source for RuntimeBus route-status queries and may show intermediate states such as `Starting`.
_Avoid_: controller state, live route state

**Route Lifecycle Compensation**:
Control-plane recovery outcome recorded when a lifecycle side effect fails after intent or state was persisted. The Route is marked `Failed` rather than silently rolling back accepted history.
_Avoid_: rollback, undo

**ErrorHandlerConfig**:
Runtime representation of an ErrorHandler — the compiled form of the DSL `ErrorHandler` declaration.
Contains `ExceptionPolicy` list, `DeadLetterChannel` URI, and retry settings applied by the error handler Tower layer.
_Avoid_: error handler (unqualified, use DSL term when describing declaration; use this term when describing runtime type)

**ExceptionPolicy**:
Runtime per-error matching rule compiled from a DSL `OnException`. Uses a predicate over
`CamelError` to select which errors trigger this policy's retry/routing behaviour.
_Avoid_: OnException (use OnException in DSL context; ExceptionPolicy in runtime context)

## Compiled Step Variants

**Process**:
The most common variant — wraps a `BoxProcessor` (a Tower `BoxCloneService<Exchange, Exchange, CamelError>`). Used for all non-structural EIP steps (setBody, log, marshal, etc.).

**Stop**:
Terminates route processing successfully. `run_steps` converts it to `PipelineOutcome::Stopped(ex)` without invoking any Tower service. The exchange state is preserved as-is; the reply channel sees `Ok(ex)` (indistinguishable from Completed).

**Segment**:
Wraps an `OutcomeSegment` — a structural EIP sub-pipeline (Filter, Choice, Loop, Throttle, doTry, Split, StreamingSplit, Multicast, LoadBalance) that returns `PipelineOutcome` directly. Enables `Stopped(ex)` propagation with nested-before-Stop mutations intact.

## ADR-0012 log-policy annotations

| File | Line | Category | Reason |
|------|------|----------|--------|
| `src/lifecycle/application/commands.rs` | 161 | `system-broken` | Control-plane inconsistency — persist + rollback both failed |

## Example dialogue

> "When a Consumer submits an Exchange, what happens next?"
> "The Runtime wraps it in a UnitOfWorkConfig layer and passes it to the Route's Pipeline. Each Processor transforms or routes it in sequence. When the last Processor completes, the UnitOfWorkConfig fires its completion hooks."
>
> "What is the difference between Exchange and Message?"
> "Exchange is the envelope — it holds two Messages (input and optional output), properties, the exchange pattern, and error state. Message is just body+headers. Components build the initial input Message; Processors read and write it through exchange.input."
>
> "And if a Processor fails?"
> "The failure hooks fire instead. The Exchange carries the error state — the ExchangePattern determines whether a reply with the error is sent back to the caller."
