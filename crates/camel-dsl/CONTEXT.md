# DSL

The declarative route-authoring layer. Parses YAML/JSON configuration into Routes that the
Runtime can execute. The fluent Rust API lives in camel-builder.

## ADR-0012 log-policy sites

All three sites in this crate are **system-broken** — DSL compile/parse failures that occur
before any route ErrorHandler exists. The `error!` level is preserved; every call site carries
a `// log-policy: system-broken` annotation.

| File | Line | Category | Annotation |
|------|------|----------|------------|
| `yaml.rs` | ~101 | system-broken | `// log-policy: system-broken` — YAML parse failure (`parse_yaml_to_declarative_inner`) |
| `yaml.rs` | ~138 | system-broken | `// log-policy: system-broken` — YAML parse failure (`extract_rest_blocks`) |
| `yaml.rs` | ~1787 | system-broken | `// log-policy: system-broken` — file read failure (`load_from_file`) |

> Line numbers are illustrative; the `// log-policy: system-broken` annotation is the durable
> marker. Re-verify with `rg -n '// log-policy' src/yaml.rs`.

## `#[non_exhaustive]` posture (crate-local)

ADR-0049 scopes the workspace `#[non_exhaustive]` policy to the three contract crates only
(`camel-api`, `camel-component-api`, `camel-language-api`); `camel-dsl` is out of that ADR's
scope by design. This table records the crate-local posture for the DSL authoring types, using
ADR-0049 §Rule 3 as the decision framework — **not** an extension of ADR-0049's mandatory scope.

**Posture:** an authoring type is `#[non_exhaustive]` when it is (a) an enum-taxonomy that grows
parity-driven with upstream Camel EIP coverage **and** (b) has external match or struct-literal
construction sites today (the same additive-becomes-breaking surface ADR-0049 targets for contract
enums). All six DSL authoring types below meet both tests (6 struct-literal sites in `camel-test`),
so all six are `#[non_exhaustive]: yes`.

| Type | Kind | non_exhaustive | Rationale (ADR-0049 §Rule 3 framework) |
|------|------|----------------|----------------------------------------|
| `DeclarativeRoute` | struct | yes | Root authoring struct; external struct-literal construction — new field is additive only under `#[non_exhaustive]` |
| `DeclarativeStepKind` | enum | yes | Step taxonomy; grows parity-driven with EIP coverage; externally matched |
| `DeclarativeStep` | enum | yes | Step wrapper taxonomy; externally matched alongside `DeclarativeStepKind` |
| `DeclarativeSecurityPolicy` | enum | yes | Policy-form taxonomy (`roles`/`scopes`/`ref`/`wasm`/`permission`); grows parity-driven; externally matched |
| `DeclarativeConcurrency` | enum | yes | Concurrency-form taxonomy; externally matched; compiles to `ConcurrencyModel` |
| `RouteDslStep` | enum | yes | DSL-builder step taxonomy; grows parity-driven; externally matched |

> **Scope note:** This posture is crate-local. It does **not** amend ADR-0049 or add camel-dsl to
> its binding scope; it applies the same §Rule 3 reasoning by choice. The mechanical attribute
> application (adding `#[non_exhaustive]` to the six types) is tracked in the code stream
> (rc-3pw3), not by this documentation record.

## Language

**parameters map**:
The `parameters:` authoring surface on from/to/wire_tap/enrich/poll_enrich. On the AST it is
held as a raw `BTreeMap<String, String>` (keys are NOT validated at deserialization beyond
string-ness — non-string values are rejected naming the key). The merge into the URI happens
at lowering (`yaml.rs`, via `camel_api::EndpointUri::try_from_uri_and_params`): an empty map
passes the URI through byte-identical; a key overlapping the query string or (for enrich full
form) the inner config map fails closed with `EndpointUriError::DuplicateKey`.
_Avoid_: endpoint options (that is the lint/runtime concept), query params

**RouteDefinition**:
The structured representation of a Route — produced by RouteBuilder or by parsing a YAML/JSON file. CamelContext consumes RouteDefinitions to build and start Routes.
_Avoid_: route spec, route config, route descriptor

**CanonicalRouteSpec**:
Versioned stable minimal Route contract used by runtime commands, config tooling, and hot-reload paths. v2 adds lifecycle metadata (`auto_startup`, `startup_order`, `concurrency`). Unsupported fields are strictly rejected (no silent loss); lossy escape hatch via `allow_loss` parameter. Not a full RouteDefinition mirror. (ADR-0011, ADR-0016)
_Avoid_: route definition, full DSL model

### Runtime authority: RouteDefinition is the source of truth

`RouteDefinition` is the runtime source of truth. The normal start/hot path compiles declarative DSL
straight to `RouteDefinition` and then to a compiled Pipeline (`compile_declarative_route` →
`RouteDefinition`; used by `yaml.rs` route loading and the template materializer). The controller hot
path compiles `RouteDefinition` directly (`CompileRouteDefinition { definition: RouteDefinition }`).

`CanonicalRouteSpec` is the stable, minimal **contract** for runtime commands, config tooling, and
hot-reload (the `compile_declarative_route_to_canonical` path, gated by `allow_loss`). It is **not**
the compile target of the normal route-start path — declarative DSL does **not** have to pass through
canonical to run. `RuntimeCommand` registration accepts a `CanonicalRouteSpec` but immediately lowers
it to a `RouteDefinition`. All runtime compilation still consumes `RouteDefinition`. This paragraph
is the answer to the canonical-vs-declarative authority question (ADR-0011, ADR-0016, ADR-0026).

**auto_startup**:
RouteDefinition flag, default `true`. When `false`, CamelContext registers the Route but does not start its Consumer during `CamelContext::start()`; the Route must be started through RuntimeBus, RouteController, or ControlBus.
_Avoid_: lazy route (informal), disabled route

**startup_order**:
RouteDefinition ordering key. Auto-start Routes start in ascending `startup_order`; shutdown runs in reverse order.
_Avoid_: priority, dependency order

**RouteBuilder**:
The fluent Rust API for constructing a RouteDefinition programmatically. Lives in
**camel-builder**, not this crate — see `crates/camel-builder/CONTEXT.md` for its glossary
entry and design notes. Referenced here only because the declarative YAML/JSON authoring form
in camel-dsl (`RouteDslRoute`) lowers to the same `RouteDefinition`.
_Avoid_: builder, route factory, DSL builder

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

**REST DSL**:
Declarative `rest:` blocks that lower to `http:` consumer routes with JSON binding, path templates, and optional schema validation. REST v1 supports only `application/json` for both `consumes` and `produces`; any other value fails route load. For binary, streaming, or non-JSON proxy APIs, use `http:` (which supports `Body::Stream`) instead of `rest:`. A future v2 may lift the restriction.
_Avoid_: REST API, REST endpoint (use REST DSL for the authoring form)

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
