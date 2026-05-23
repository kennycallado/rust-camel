# Runtime

The execution engine for rust-camel. Owns Exchange lifecycle, Route management, and the registries that wire together Components, Languages, Functions, and Services.

## Language

**Exchange**:
The unit of data flowing through a Route — an envelope carrying a body, headers, properties, an exchange pattern, and error state.
_Avoid_: message, event, request

**ExchangePattern**:
Declares whether an Exchange expects a reply. `InOnly` = fire-and-forget; `InOut` = request-reply.
_Avoid_: MEP (use the enum variant names directly)

**Route**:
A named message-processing pipeline: a source endpoint that emits Exchanges and a sequence of steps that transform or route them.
_Avoid_: flow, pipeline (when referring to the Route as a whole)

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

## Example dialogue

> "When a Consumer submits an Exchange, what happens next?"
> "The Runtime wraps it in a UnitOfWorkConfig layer and passes it to the Route's Pipeline. Each Processor transforms or routes it in sequence. When the last Processor completes, the UnitOfWorkConfig fires its completion hooks."
>
> "And if a Processor fails?"
> "The failure hooks fire instead. The Exchange carries the error state — the ExchangePattern determines whether a reply with the error is sent back to the caller."
