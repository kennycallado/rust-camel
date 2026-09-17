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

**route discovery (env-injected)**:
`discover_routes_with_threshold_security_and_env(patterns,
stream_cache_threshold, security_ctx, env_lookup)`: the hermetic discovery
entry — every `${env:NAME}` placeholder resolves through the injected
`env_lookup` closure, never the process environment. It preserves the full
discovery contract of the process-environment entries: glob pattern
handling, the reserved test-suffix gate, JSON explicit-pattern gating, file
size caps, two-pass template materialization, stream-cache threshold
threading, and security compile-context threading. The integration tier
injects its layered environment here (ADR-0069 section 4).
_Avoid_: env-aware discovery (the process-environment entries are the
default; this entry is the hermetic variant)

**file route loading (env-interpolated)**:
`load_from_file` resolves `${env:NAME}` placeholders with the same
tree-walk-first strategy as discovery's YAML arm (`interpolate_yaml_source`,
rc-93wct): the parsed tree is interpolated scalar-by-scalar — comments are
never interpolated, and a substituted leaf keeps STRING typing at the
interpolation seam (wave-E canon). The provenance layer
(`fn interpolate_yaml_source_with_provenance`, `env_interpolation.rs:120`)
records every whole-scalar substituted leaf as a STRUCTURAL path
(`enum ProvenanceSeg`, `Key`/`Index` segments, `env_interpolation.rs:188`);
embedded tokens, mapping keys, and `$$` escapes are never provenance. The
typed probe (`fn parse_with_probe`, `env_int_probe.rs:152`) runs only AFTER
a failed typed parse: candidates are provenance paths under the top-level
`routes:` key whose value passes `fn clean_integer` (`env_int_probe.rs:44`;
lexical `-?(0|[1-9][0-9]*)`, then an exact i64-or-u64 parse), and candidate
index subsets are tried smallest-first in document order (cap 8,
`MAX_PROBE_CANDIDATES`) against a QUIET parser oracle — first success wins,
otherwise the original first-pass error stands. A placeholder on an
int-typed field (e.g. `throttle.max_requests`) therefore LOADS, coerced to
the number; string and polymorphic positions keep string typing. Scope is
routes-only: REST blocks, route templates, and the legacy whole-text
fallback (which carries no provenance) are never probed. The public seam is
`fn parse_routes_with_env` (`yaml.rs:2252`) plus its typed
`enum RoutesEnvError` (`yaml.rs:2211`) — callers distinguish
`Unresolved(var)` from `Parse(..)` BY TYPE. On final failure the seam
replays the text through the logging parser exactly once. Discovery's YAML
arm shares the same probe (`fn discover_routes_inner`, `discovery.rs:301`),
wrapping a quiet threshold/security twin
(`fn parse_yaml_with_threshold_and_security_quiet`, `yaml.rs:289`). The
template arm reads a narrowed
`struct TemplateSections` view (`template/yaml.rs:24`) that no longer
re-validates `routes`/`rest`/`mcp` — a behavior loosening for out-of-tree
callers; bd rc-28b90 tracks the archive-time review. A `:-default` token resolves to the default, and an
unset variable without a default fails the load naming the variable
(`Environment variable 'NAME' not set (required by <path>)` — the same
wording the LEAN runner surfaces as its doc error). The process environment
is never read; the lookup-injectable variant
`load_from_file_with_env(path, lookup)` is the injection point for
ambient values (the rest-block sibling
`extract_rest_blocks_from_file_with_env` carries the same lookup seam for
the `camel openapi generate` surface). Escapes (`$$`, `$${env:...}`)
apply; both entries keep the 16 MiB cap and path-annotated parse errors.
_Avoid_: env-aware loading, pre-parse substitution (the tree walk
interpolates parsed scalars; only the fallback splices raw text)

**virtual document store (`VirtualDocumentStore`)**:
The canonical in-memory pack of compile-time documents for v2 compiled
artifacts (ADR-0075). Public `embedded_store` module
(`src/embedded_store.rs`): `VirtualDocumentStore` (content blob plus
validated `StoreIndex`), `StoreEntry`, `StoreEntryKind`
(`route`/`job`/`config`/`include`/`profile`), `SourcePlan`, and
`StoreError`. Entry paths are normalized UTF-8 relative `/` paths;
entries sit in canonical (lexicographic) path order for stable bytes,
while the `SourcePlan` preserves declared route-source order as the
normative array order. Build and decode are fail-closed: unknown
`STORE_SCHEMA`, duplicate paths, noncanonical order, out-of-bounds or
overlapping ranges, unreferenced content, and missing references are
named `StoreError`s, never reinterpreted. `camel-cli` re-exports this
model verbatim; no second store model exists.
_Avoid_: embedded bundle, artifact archive, virtual FS

**discover_virtual_store**:
The filesystem-free discovery seam for embedded stores
(`discovery.rs`): accepts a `VirtualDocumentStore` and a deployment
`env_lookup` closure, and returns `VirtualStoreDiscovery` — the merged
configuration tree plus route definitions in plan order. Configuration
assembly is typed and ordered: the indexed `Camel.toml`, include, and
selected-profile texts merge in index order (includes lowest, then the
configuration document, profile-section selection per document), mirroring
the canonical ordering in `camel_dsl::config_semantics` (bd rc-io2zl).
Only source-plan references resolve, strictly by index lookup, through
the shared interpolation, typed env probing, template materialization,
reserved-document validation, parsing, and lowering paths — the same
semantics as filesystem discovery. The seam NEVER touches the
filesystem: no discovery, no glob expansion, no canonicalization, no
temporary-file helpers. A file placed beside the artifact after
compilation is invisible because the store is the only document source.
Invalid embedded TOML or violated merge rules surface as
`MalformedVirtualConfig`; missing or mistyped references surface as the
named `StoreError`s. Authority: ADR-0075.
_Avoid_: embedded discovery (ambiguous), runtime discovery (this seam
forbids exactly that)

**`compiled://` provenance**:
The virtual source identity of a store document: a diagnostic for the
entry at logical path `routes/orders.yaml` names
`compiled://routes/orders.yaml`. Document boundaries and provenance
survive without filesystem paths; every parse or interpolation error on
an embedded document reports this identity. Authority: ADR-0075.
_Avoid_: source file path (an embedded document has none at runtime)

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
Declarative `rest:` blocks that lower to `http:` consumer routes with JSON binding, path templates, and optional schema validation. Binding defaults to `json`: requests unmarshal and responses marshal automatically, JSON Schema validation runs when schemas are declared, and JSON-essence media types are accepted (bare `application/json`, parameterized forms like `application/json; charset=utf-8`, and `+json` structured-syntax suffixes like `application/problem+json`). Explicit `binding: raw` accepts any RFC 9110 type/subtype media type, performs no automatic unmarshal/marshal, leaves the request as `Body::Stream`, and sends the trimmed `produces` value as the response Content-Type. `request_schema` and `response.schema` are rejected in raw mode. Lowering also injects a media negotiation gate (`src/media.rs` + `ContentNegotiationProcessor`) as the first step: declared `consumes`/`produces` are enforced with 415 (unsupported request Content-Type; body-less verbs skip) and 406 (Accept media-range precedence; equal-specificity ties take the lowest q); absent headers and undeclared sides are permissive; a malformed Accept header degrades to `*/*` while a malformed Content-Type fails closed.
_Avoid_: REST API, REST endpoint (use REST DSL for the authoring form)

**SecurityPolicy**:
Route-level authorization declaration evaluated before normal Route Steps. DSL config must choose exactly one form: `roles`, `scopes`, `ref`, `wasm`, or `permission`; grants attach a Principal to the Exchange, denials return `Unauthorized` into route error handling. Downstream Route Steps do not run unless error handling routes or handles the error.
_Avoid_: authentication config, ACL, policy step

**Concurrency**:
Route-level override for processing Exchanges sequentially or concurrently, with an optional
maximum parallelism. Compiles to `ConcurrencyModel`. Declared as `sequential: true` or
`concurrent: 8` (integer max) in YAML.
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
