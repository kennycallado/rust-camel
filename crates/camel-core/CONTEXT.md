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

**Pipeline**:
The compiled assembly of Processors that processes an Exchange through a Route's data plane.
_Avoid_: chain, middleware stack

**Processor**:
A single processing unit in a Pipeline — either an EIP pattern (filter, choice, split, setBody) or any custom step that receives and returns an Exchange.
_Avoid_: handler, middleware, transformer

**CamelContext**:
The composition root of the runtime. Manages component, language, function, and service registries; controls Route lifecycle.
_Avoid_: container, application context, context (unqualified)

**UnitOfWorkConfig**:
Optional per-Route configuration that wraps the Pipeline with exchange lifecycle tracking. When present, fires completion or failure hooks (by producer URI) when an Exchange exits the Pipeline.
_Avoid_: transaction, scope, UnitOfWork

**RuntimeBus**:
CQRS façade for the Route control plane. Commands mutate Route state (start, stop, add); queries read current status from projections.
_Avoid_: event bus, message bus, command bus (unqualified)

**ErrorHandlerConfig**:
Runtime representation of an ErrorHandler — the compiled form of the DSL `ErrorHandler` declaration.
Contains `ExceptionPolicy` list, `DeadLetterChannel` URI, and retry settings applied by the error handler Tower layer.
_Avoid_: error handler (unqualified, use DSL term when describing declaration; use this term when describing runtime type)

**ExceptionPolicy**:
Runtime per-error matching rule compiled from a DSL `OnException`. Uses a predicate over
`CamelError` to select which errors trigger this policy's retry/routing behaviour.
_Avoid_: OnException (use OnException in DSL context; ExceptionPolicy in runtime context)

## Example dialogue

> "When a Consumer submits an Exchange, what happens next?"
> "The Runtime wraps it in a UnitOfWorkConfig layer and passes it to the Route's Pipeline. Each Processor transforms or routes it in sequence. When the last Processor completes, the UnitOfWorkConfig fires its completion hooks."
>
> "What is the difference between Exchange and Message?"
> "Exchange is the envelope — it holds two Messages (input and optional output), properties, the exchange pattern, and error state. Message is just body+headers. Components build the initial input Message; Processors read and write it through exchange.input."
>
> "And if a Processor fails?"
> "The failure hooks fire instead. The Exchange carries the error state — the ExchangePattern determines whether a reply with the error is sent back to the caller."
