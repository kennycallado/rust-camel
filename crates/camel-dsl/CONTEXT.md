# DSL

The route definition layer. Provides a fluent builder API and YAML/JSON configuration format for constructing Routes that the Runtime can execute.

## Language

**RouteDefinition**:
The structured representation of a Route — produced by RouteBuilder or by parsing a YAML/JSON file. CamelContext consumes RouteDefinitions to build and start Routes.
_Avoid_: route spec, route config, route descriptor

**RouteBuilder**:
Fluent Rust API for constructing a RouteDefinition programmatically using method chaining.
_Avoid_: builder, route factory

**Step**:
A single processing instruction in a RouteDefinition (e.g., `setBody`, `filter`, `to`, `choice`). Steps compile into Processors in the Runtime Pipeline.
_Avoid_: instruction, action, operation

**from**:
The source URI declaration that opens a RouteDefinition — identifies the Component and Endpoint that will produce Exchanges for the Route (e.g., `timer:tick`, `kafka:my-topic`).
_Avoid_: source, input, consumer URI (in DSL context)

**to**:
A Step that sends an Exchange to an Endpoint URI (e.g., `log:info`, `http:my-service`).
_Avoid_: sink, destination (when used as a DSL term)

**ErrorHandler**:
Per-Route error handling strategy declared in a RouteDefinition. Retries failed Exchanges and/or routes them to a DeadLetterChannel. Compiles to `ErrorHandlerConfig` in the Runtime.
_Avoid_: exception handler, failure handler, DeclarativeErrorHandler (unless naming the Rust struct)

**OnException**:
Per-exception-class clause inside an ErrorHandler, with optional RedeliveryPolicy, handled flag, and handler Steps or handled-by URI. Compiles to `ExceptionPolicy` in the Runtime.
_Avoid_: catch block, exception rule, DeclarativeOnException

**DeadLetterChannel**:
ErrorHandler destination URI that receives Exchanges after processing fails or all redelivery attempts are exhausted.
_Avoid_: dead letter queue, DLQ (unless the external system is specifically a queue), dead_letter_channel (unless naming the YAML field)

**RedeliveryPolicy**:
Retry configuration inside an ErrorHandler or OnException: maximum attempts, delay, backoff
multiplier, max delay, jitter, and optional `handled_by` URI (route to this URI after exhausting
retries instead of propagating the error).
_Avoid_: retry settings, backoff config, DeclarativeRedeliveryPolicy

**CircuitBreaker**:
Route-level resilience configuration that opens after repeated failures and temporarily rejects Exchanges before the Step pipeline runs. Not a Step — declared at the RouteDefinition level.
_Avoid_: breaker, failure gate, DeclarativeCircuitBreaker

**Concurrency**:
Route-level override for processing Exchanges sequentially or concurrently, with an optional
maximum parallelism. Compiles to `ConcurrencyModel`. Declared as `sequential: true` or
`concurrent: { max: N }` in YAML.
_Avoid_: threading mode, parallelism setting, DeclarativeConcurrency

**UnitOfWork** (YAML hooks):
Optional route-level hooks `on_complete` and `on_failure` (producer URIs) that fire when an
Exchange exits the Pipeline successfully or with an error. Compiles to `UnitOfWorkConfig`.
_Avoid_: transaction hooks, lifecycle hooks (use UnitOfWork in DSL context)

## Example dialogue

> "I want to read from Kafka and send to HTTP."
> "Define a RouteDefinition using RouteBuilder: start with `from('kafka:my-topic')`, add any transformation Steps, then end with `to('http:my-service')`."
>
> "Can I define the same route in YAML?"
> "Yes — the YAML parser produces the same RouteDefinition. The Runtime doesn't know or care which form was used."
>
> "What is the difference between ErrorHandler and OnException?"
> "ErrorHandler is the per-Route strategy: it decides what happens when any Step fails — retry N times, then send to DeadLetterChannel. OnException scopes that behaviour to a specific exception class. You can have one ErrorHandler with multiple OnException clauses."
>
> "Is CircuitBreaker a Step I add to the pipeline?"
> "No — CircuitBreaker is a route-level config, not a Step. It wraps the entire Pipeline and opens before any Step runs if recent failures exceed the threshold."
