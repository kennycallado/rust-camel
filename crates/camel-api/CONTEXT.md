# API Contracts

The pure contract crate. Defines the canonical **data types** and **EIP/trait abstractions** that
every other crate depends on — Exchange, Message, Body, CamelError, the Tower `Processor` shape, the
`PipelineOutcome` layer, and the CQRS runtime bus traits. It owns no runtime behavior: it has no
execution engine, no registries, no lifecycle implementation.

> **Scope boundary.** This file is the **type-definition glossary** — the stable contract vocabulary.
> The **behavioral vocabulary** (how these types are executed, the Route lifecycle, RuntimeBus
> semantics, registries) lives in the Runtime context, [`crates/camel-core/CONTEXT.md`](../camel-core/CONTEXT.md).
> When a term means "the shape of the data/contract", define it here; when it means "what the
> runtime does with it", define it in camel-core and cross-link.

## Language

**Exchange**:
The canonical data envelope (`exchange.rs`). Carries an input `Message`, an optional output
`Message`, properties, extensions, error state, an `ExchangePattern`, and optional tracing context.
This crate defines the *type*; the Runtime defines its *lifecycle* (see camel-core CONTEXT.md).
_Avoid_: message (for the whole envelope), event, request, packet

**Message**:
The body+headers container inside an Exchange (`message.rs`). `exchange.input` is always present;
`exchange.output` is set for `InOut` replies.
_Avoid_: exchange, payload (the payload is the Message *body*, not the Message), envelope

**Body**:
The payload of a Message (`body.rs`) — bytes, a typed value, or a stream (`StreamBody`).
_Avoid_: content, data, payload (when referring to the whole Message)

**ExchangePattern**:
Enum declaring whether an Exchange expects a reply: `InOnly` (fire-and-forget) or `InOut`
(request-reply).
_Avoid_: MEP, message exchange pattern (use the variant names)

**CamelError**:
The crate-wide error enum (`error.rs`). Variants include domain failures plus control signals like
`ConsumerStopping`. Note: `Stopped` is NOT here — Stop is modeled as `PipelineOutcome::Stopped`
above Tower (ADR-0024).
_Avoid_: error type, failure, exception (use CamelError for the enum)

**Processor**:
The blanket trait (`processor.rs:15`) over every Tower
`Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + Sync + 'static`. The
universal "one processing step" contract.
_Avoid_: handler, middleware, transformer, step (Step is a DSL/compiled concept, not this trait)

**BoxProcessor**:
The runtime-erased processor type (`processor.rs:47`):
`tower::util::BoxCloneService<Exchange, Exchange, CamelError>`. The composable unit a Pipeline is
built from. `SyncBoxProcessor` wraps it for `Sync` contexts.
_Avoid_: boxed service, dyn processor

**PipelineOutcome**:
Enum (`pipeline_outcome.rs`): `Completed(Exchange) | Stopped(Exchange) | Failed(CamelError)`. Sits
**one layer above Tower** — Tower responses stay `Result<Exchange, CamelError>`. Defined here;
produced by the executor in camel-core. Established by ADR-0024.
_Avoid_: pipeline result, route outcome, step result

**OutcomePipeline / OutcomeSegment**:
`OutcomePipeline` (`outcome_pipeline.rs`) is the trait for structural EIP sub-pipelines that return
`PipelineOutcome` directly; `OutcomeSegment` (`outcome_segment.rs`) is the wrapper struct with
tracing/metrics hooks. Composition helpers live in camel-core. Established by ADR-0025.
_Avoid_: outcome service, segment processor (Segment is the `CompiledStep` variant in camel-core)

**RuntimeCommandBus / RuntimeQueryBus**:
The CQRS control-plane contract traits (`runtime.rs`). Commands mutate route lifecycle; queries
observe it. This crate defines the *ports*; camel-core's `RuntimeBus` implements them. Established by
ADR-0002.
_Avoid_: runtime bus (RuntimeBus is the camel-core implementation, not this trait pair), event bus

**CanonicalRouteSpec**:
The versioned, stable, minimal route contract (`runtime.rs`) used by runtime commands, config
tooling, and hot-reload. Distinct from the full `RouteDefinition` (DSL context). Established by
ADR-0011 / ADR-0016.
_Avoid_: RouteDefinition (that is the full DSL model), route config

**SecurityPolicy / Principal / AuthorizationDecision**:
Route-level authorization contract types (`security_policy.rs`). Defined here so camel-core and
camel-dsl can reference them without depending on the auth service. The decision *sources* (JWT,
permission engines) live in `camel-auth`. Established by ADR-0010.
_Avoid_: auth service, authenticator (those are camel-auth concepts, not these contract types)

**OptionKind**:
Closed enum of supported URI option value types (String, Int, Bool, Float, Duration, Enum, List).
Defined in `component_metadata.rs`.
_Avoid_: option type, parameter type

**UriOption**:
A single URI-parameter definition with builder methods. Includes name, description, kind, required
flag, default value, aliases, deprecation notice, and secret flag. Defined in
`component_metadata.rs`.
_Avoid_: parameter, query param

**ComponentCapabilities**:
Named boolean flags declaring what a component supports (consumer, producer, polling_consumer,
streaming). Defined in `component_metadata.rs`.
_Avoid_: feature flags, component flags

**CapabilityQuery**:
Tri-state query struct for filtering components by capability. Each field is `Option<bool>` —
`None` means "don't care". Defined in `component_metadata.rs`.
_Avoid_: filter, search query

**ComponentMetadata**:
Top-level component descriptor: scheme, schema_version, version, description, uri_syntax,
capabilities, and uri_options. The stable contract that SDK emits, registry indexes, catalog
displays, and tooling consumes. Defined in `component_metadata.rs`.
_Avoid_: component info, component descriptor (use ComponentMetadata for the type)

**ComponentMetadataCatalog**:
Trait defining the query interface for runtime component metadata lookup. Returns owned
`ComponentMetadata` values (references can't span Mutex guards). Defined in `component_metadata.rs`,
implemented by `RuntimeComponentMetadataCatalog` in camel-core.
_Avoid_: metadata store, metadata service

## Example dialogue

> "Where is `Exchange` defined, and where is its lifecycle?"
> "The `Exchange` type is defined here in camel-api (`exchange.rs`). What happens to it at runtime —
> the UnitOfWork wrap, Route pipeline execution, completion hooks — is behavioral vocabulary and
> lives in camel-core's CONTEXT.md."
>
> "Why is `Stopped` not a `CamelError` variant?"
> "Because Stop is successful control flow, not an error. It is `PipelineOutcome::Stopped`, which
> sits one layer above Tower. Tower `Service<Exchange>` responses stay `Result<Exchange, CamelError>`.
> See ADR-0024."
>
> "I need to add a route-level auth check. Which crate owns the types?"
> "The contract types (`SecurityPolicy`, `Principal`, `AuthorizationDecision`) are here in camel-api.
> The enforcement boundary (`SecurityPolicyLayer`) is in camel-core; the decision sources (JWT, OIDC,
> permission engines) are in camel-auth."
