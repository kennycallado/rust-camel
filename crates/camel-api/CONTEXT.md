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

## `#[non_exhaustive]` posture

ADR-0049 places this contract crate in its mandatory scope: every `pub enum` is
`#[non_exhaustive]` or carries an `exhaustive-by-contract` exception note. The 56 `pub enum`s
split into:

| Category | Count | Posture |
|---|---|---|
| `#[non_exhaustive]` | 52 | 48 attributed by rc-3pw3; pre-existing exceptions include `CamelError`, `ConfigValidationError`, `TemplateError`. |
| `exhaustive-by-contract` exception | 4 | `PipelineOutcome` (the ADR-0024 outcome algebra), `ExchangePattern` (the fixed InOnly/InOut MEP dichotomy), `ContentType` (the closed 4-variant cache content-type set matched by out-of-crate CacheService), and `CredentialSource` (the closed credential-source set; out-of-crate camel-auth extraction matches all variants). Each carries a `/// exhaustive-by-contract:` rustdoc note and stays exhaustive. |

New contract enums use `#[non_exhaustive]` from birth; a closed-set exception needs a
`/// exhaustive-by-contract: <rationale>` note. ADR-0049 Rule 3 governs public structs (out of
the enum mandate). Compliance is enforced by `cargo xtask lint-non-exhaustive`.

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
The blanket trait (`trait Processor`) over every Tower
`Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + Sync + 'static`. The
universal "one processing step" contract.
_Avoid_: handler, middleware, transformer, step (Step is a DSL/compiled concept, not this trait)

**BoxProcessor**:
The runtime-erased processor type alias:
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

**UriOptionMatch**:
`#[non_exhaustive]` enum describing how a `UriOption` with `pattern: Some(_)` matches URI query
keys. Initial variant: `Prefix { separator: String }`. The `#[non_exhaustive]` posture follows
ADR-0049 (forward-compat: future variants like `Glob` or `Regex` are additive). Defined in
`component_metadata.rs`.
_Avoid_: pattern matcher, match type

**UriOption.pattern**:
Optional `pattern: Option<UriOptionMatch>` field on `UriOption`. When `Some`, the option matches
URI query keys by prefix instead of by exact name; the `name` field becomes a documentation anchor
and does not participate in matching. Serialized with
`#[serde(default, skip_serializing_if = "Option::is_none")]` — absent for legacy options, so
existing JSON output is byte-identical. Defined in `component_metadata.rs`.
_Avoid_: match field, namespace flag

**UriOption::pattern_prefix**:
Consuming builder `UriOption::pattern_prefix(separator: &str) -> Self` that sets
`pattern: Some(UriOptionMatch::Prefix { separator: separator.to_string() })`. Mirrors the
`secret`/`required`/`deprecated` builder shape. Defined in `component_metadata.rs`.
_Avoid_: with_pattern, set_pattern

**IdempotentRepository**:
Key-only pluggable store for the Idempotent Consumer EIP (`trait IdempotentRepository`). `contains()` returns
`Result<bool, CamelError>` (NOT `bool`) — backends (Redis, SQL) propagate transient failures as
`Err`, never as "not a duplicate" (Contract C1). Stores keys, not messages: it tracks *which*
messages were seen, not *what* they contained. Payload-bearing storage is `ClaimCheckRepository`, a
distinct trait. Canonical impl: `MemoryIdempotentRepository` in camel-core. Established by ADR-0023.
_Avoid_: idempotent store, dedup store, claim check (that is the payload-bearing trait)

**ClaimCheckRepository**:
Payload-bearing pluggable store for the Claim Check EIP (`trait ClaimCheckRepository`). `set`/`get` work with
whole `Message` payloads (body + headers) — NOT key-only like `IdempotentRepository`. Supports
single-value keys (`set`/`get`/`get_and_remove`) and key-scoped LIFO stacks (`push`/`pop`). The
key-only vs payload-bearing split from `IdempotentRepository` is the central decision of ADR-0028
(each pattern owns its trait; only the `NamedRegistry` wiring is shared). Canonical impl:
`MemoryClaimCheckRepository` in camel-core. Established by ADR-0028.
_Avoid_: claim store, payload cache, idempotent repository (that is the key-only trait)

**CacheRepository**:
TTL cache port for the Caching EIP (`trait CacheRepository`). Stores `CacheEntry { bytes: Vec<u8>, content_type, expires_at }` — materialized bytes, not `Body` (which is not Serialize and includes the un-cacheable `Stream` variant). `get` returns `Ok(None)` on miss OR in-band expiry (never silent on backend failure — Contract C1). `peek_stale` ignores in-band expiry. `set` computes `expires_at` from `ttl`. `invalidate_prefix` has a fail-closed default (`Err` naming the backend, never `Ok(0)`); `RedbCacheRepository` overrides it with range deletion. `stats()` returns `CacheStats` with `peek_stale_served`, `invalidations`, and `bytes: Option<u64>` (`Some(sum)` for redb, `None` for the memory backend). Distinct from `IdempotentRepository` (key-only) and `ClaimCheckRepository` (payload-owning, no expiry). Canonical impls: `MemoryCacheRepository` (moka) and `RedbCacheRepository` in camel-core. Established by ADR-0056.
_Avoid_: cache store, payload cache, idempotent repository (that is the key-only trait)

**EndpointUri**:
Authoring-boundary value type holding `scheme`, `path`, `params: BTreeMap<String,String>` and the raw query bytes. Constructed via `try_from_uri_and_params` (fail-closed on duplicate keys across query and params); `to_canonical_string` renders deterministically (raw query byte-for-byte first, params appended sorted, pinned percent-encoding); `to_redacted_string` masks secret-flagged, unresolved, and unknown-scheme options plus userinfo credentials. Never serialized across the persistence boundary; `Debug` is redacting (ADR-0051 `redacting-wrapper`).
_Avoid_: URI wrapper (vague), parsed URL (implies full RFC 3986 semantics)

**EndpointUriError**:
Typed error for `EndpointUri` construction: `DuplicateKey`, `MissingScheme`, `EmptyQueryKey`, `InvalidParamKey`. Converts into `CamelError::EndpointUri`.
_Avoid_: stringly Config errors for URI merge failures

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
