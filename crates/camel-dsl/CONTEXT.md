# DSL

The route definition layer. Provides a fluent builder API and YAML/JSON configuration format for constructing Routes that the Runtime can execute.

## Language

**RouteDefinition**:
The structured representation of a Route — produced by RouteBuilder or by parsing a YAML/JSON file. CamelContext consumes RouteDefinitions to build and start Routes.
_Avoid_: route spec, route config, route descriptor

**CanonicalRouteSpec**:
Stable minimal Route contract used by runtime commands, config tooling, and hot-reload paths. It is not a full RouteDefinition mirror; unsupported fields are rejected or reset to defaults when compiled.
_Avoid_: route definition, full DSL model

**auto_startup**:
RouteDefinition flag, default `true`. When `false`, CamelContext registers the Route but does not start its Consumer during `CamelContext::start()`; the Route must be started through RuntimeBus, RouteController, or ControlBus.
_Avoid_: lazy route (informal), disabled route

**startup_order**:
RouteDefinition ordering key. Auto-start Routes start in ascending `startup_order`; shutdown runs in reverse order.
_Avoid_: priority, dependency order

**RouteBuilder**:
Fluent Rust API for constructing a RouteDefinition programmatically using method chaining.
_Avoid_: builder, route factory

**RouteTemplate**:
Parameterized DSL definition that expands into one or more RouteDefinitions before the Runtime sees them. Template parameters are substituted before DSL deserialization.
_Avoid_: route macro, route generator, reusable route (too vague)

**Step**:
A single processing instruction in a RouteDefinition (e.g., `setBody`, `filter`, `to`, `choice`). Steps compile into Processors in the Runtime Pipeline.
_Avoid_: instruction, action, operation

**Aggregate**:
A Step that groups Exchanges by correlation key and emits one combined Exchange when its completion condition is met (`completion_size`, timeout, or predicate when supported). Pending buckets do not continue through later Steps until completed.
_Avoid_: batch, collect (too vague), aggregation route

**StreamCache**:
Step that materializes `Body::Stream` into `Body::Bytes` up to a threshold so later Steps can reread body content. Non-stream bodies pass through unchanged.
_Avoid_: streaming mode, buffer config

**force_completion_on_stop**:
Aggregate option that emits all pending buckets when the Route stops or the Consumer exits. If false, pending buckets are discarded on exit/stop without cancelling the rest of the Pipeline solely because the Consumer ended.
_Avoid_: flush_on_timeout, drain_on_shutdown

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

**SecurityPolicy**:
Route-level authorization declaration evaluated before normal Route Steps. DSL config must choose exactly one form: `roles`, `scopes`, `ref`, `wasm`, or `permission`; grants attach a Principal to the Exchange, denials return `Unauthorized` into route error handling. Downstream Route Steps do not run unless error handling routes or handles the error.
_Avoid_: authentication config, ACL, policy step

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
