# Context Map

## Contexts

- [API Contracts](./crates/camel-api/CONTEXT.md) — pure contract crate: canonical data types (Exchange, Message, Body, CamelError) and EIP/trait abstractions (Processor, BoxProcessor, PipelineOutcome, CQRS bus traits, CanonicalRouteSpec). Type-definition site; behavioral vocabulary lives in Runtime.
- [Runtime](./crates/camel-core/CONTEXT.md) — core execution engine: Exchange lifecycle, Route management, and component/language/function/service registries
- [DSL](./crates/camel-dsl/CONTEXT.md) — route definition via fluent builder API and YAML/JSON configuration files (ADR-0026)
- [Components](./crates/components/CONTEXT.md) — inbound/outbound adapters (timer, HTTP, Kafka, file, etc.) that feed Exchanges into Routes and send them to external systems
  - [LLM Component](./crates/components/camel-component-llm/CONTEXT.md) — LLM chat and embedding component (OpenAI, Ollama, Mock) with streaming and materialized modes
  - [File Component](./crates/components/camel-file/CONTEXT.md) — polls directories and writes exchange bodies to disk; `atomic_write` helper powers `Override`/`TryRename` write strategies (`Fail` uses `create_new(true)` directly — already atomic)
- [Languages](./crates/languages/CONTEXT.md) — expression and predicate evaluation against Exchanges (JavaScript, JSONPath, XPath, Simple, Rhai)
  - [Language SPI](./crates/languages/camel-language-api/CONTEXT.md) — the trait contract every language implements (Language, Expression, Predicate, MutatingExpression/Predicate) and `LanguageError`
- [Functions](./crates/services/camel-function/CONTEXT.md) — out-of-process executable units invoked as pipeline steps; inspired by serverless functions, running in isolated containers
- [Services](./crates/services/CONTEXT.md) — cross-cutting infrastructure services: observability (OTel, Prometheus), auth/security, and platform integration (Kubernetes)
  - [Auth Service](./crates/services/camel-auth/CONTEXT.md) — provider-neutral token validation, claim mapping, and permission evaluation (decision sources behind the route SecurityPolicy boundary)

## Relationships

- **DSL → Runtime**: DSL compiles route definitions into `RouteDefinition` objects consumed by `CamelContext`
- **Components → Runtime**: Components register by URI scheme into `CamelContext`; Consumers submit `ExchangeEnvelope` into Route pipelines
- **Runtime → Components**: Runtime resolves a Component by scheme, creates an Endpoint, and starts a Consumer for each Route's `from:` URI
- **Languages → Runtime**: Language implementations register into `CamelContext`; the runtime resolves them to evaluate expressions and predicates within Pipeline steps
- **Functions → Runtime**: A `FunctionInvoker` is registered in `CamelContext`; the `function:` Pipeline step calls it with an Exchange and applies the returned patch
- **Services → Runtime**: Services implement `Lifecycle`, `MetricsCollector`, or `PlatformService` contracts and register into `CamelContext` for coordinated start/stop

## Architecture Decisions

Cross-cutting decisions that shaped the architecture live in [`docs/adr/`](./docs/adr/).
Each ADR file carries authoritative `Status` / `Amends` metadata in its header; the markers below
are a convenience index and may lag the file.

- [0001](./docs/adr/0001-tower-data-plane-split-from-control-plane.md) — Tower data plane, custom-trait control plane
- [0002](./docs/adr/0002-cqrs-runtime-bus-for-route-lifecycle.md) — CQRS RuntimeBus for route lifecycle control
- [0003](./docs/adr/0003-hexagonal-lifecycle-core.md) — Hexagonal architecture for camel-core lifecycle
- [0004](./docs/adr/0004-hot-reload-atomic-pipeline-swap.md) — Hot reload via atomic pipeline swap (ArcSwap snapshot isolation)
- [0005](./docs/adr/0005-function-out-of-process-staged-reload.md) — `function:` out of process with staged registration
- [0006](./docs/adr/0006-script-synchronous-boa-async-to-function.md) — `script:` synchronous; async JS delegated to `function:`
- [0007](./docs/adr/0007-route-supervised-consumer-failure.md) — Route-supervised Consumer failure: route crash → CrashNotification → RuntimeBus FailRoute → route enters Failed state. Consumer::stop() is NOT called on crash.
- [0008](./docs/adr/0008-route-templates-json-tree-substitution.md) — RouteTemplate expansion via JSON tree substitution before DSL deserialization
- [0009](./docs/adr/0009-http-co-hosting-api-and-static-routes.md) — HTTP API routes and static file mounts share one server per host/port with deterministic dispatch precedence
- [0010](./docs/adr/0010-security-policy-pre-pipeline-authorization.md) — Route-level SecurityPolicy authorizes before normal Route Steps run, rather than as a normal Pipeline Step
- [0011](./docs/adr/0011-canonical-route-spec-minimal-contract.md) — CanonicalRouteSpec v1 is a stable minimal route contract, not a full RouteDefinition mirror
- [0012](./docs/adr/0012-log-level-convention-handler-contract-boundaries.md) — Log levels follow handler-contract ownership boundaries
- [0013](./docs/adr/0013-network-retry-policy-and-migration.md) — NetworkRetryPolicy centralizes adapter retry semantics and migration boundaries
- [0014](./docs/adr/0014-wasm-plugin-config-unification.md) — WASM plugin runtime configuration is unified across plugin types
- [0015](./docs/adr/0015-endpoint-created-polling-consumer-for-pollenrich.md) — Endpoint-created PollingConsumer powers pollEnrich and WASM camel_poll
- [0016](./docs/adr/0016-canonical-route-spec-v2-contract.md) — CanonicalRouteSpec v2 adds lifecycle/execution metadata with strict rejection for unsupported fields _(amends 0011)_
- [0017](./docs/adr/0017-dsl-yaml-snake-case-naming-convention.md) — DSL keys use snake_case (YAML and JSON, amended 2026-06-26) to match Rust field names and schema output
- [0018](./docs/adr/0018-two-phase-route-lifecycle-persistence.md) — Route lifecycle commands persist intent before side effects, use optimistic versions, and compensate to Failed on side-effect failure _(amends 0002, 0003, 0004, 0007)_
- [0019](./docs/adr/0019-error-disposition-pipeline-recovery.md) — Error disposition decisions moved inside the pipeline loop via RouteErrorHandler trait injection; ExceptionDisposition enum (Propagate/Handled/Continued) replaces handled:bool
- [0020](./docs/adr/0020-llm-component-provider-adapter-boundary.md) — LLM component isolates siumai SDK behind a project-owned LlmProvider trait; all siumai imports confined to two files
- [0021](./docs/adr/0021-llm-retry-retry-after-manual-loop.md) — LLM retry honors provider retry_after via manual loop, diverging from ADR-0013 helpers
- [0022](./docs/adr/0022-steplifecycle-trait-and-drain.md) — StepLifecycle trait and drain policy for stateful pipeline steps
- [0023](./docs/adr/0023-idempotent-repository-trait.md) — `IdempotentRepository` trait lives in `camel-api` (key-only, `Result`-returning) so any crate can implement it; backends propagate transient read failures. Stores keys, not full messages (Claim Check is ADR-0028)
- [0024](./docs/adr/0024-pipeline-outcome-replaces-camel-error-stopped.md) — `PipelineOutcome` enum replaces `CamelError::Stopped` for control flow; Stop EIP becomes successful control flow at the pipeline layer (one above Tower); consumer reply-channel adapter makes Completed/Stopped indistinguishable to consumers
- [0025](./docs/adr/0025-outcome-aware-structural-eips.md) — Outcome-aware structural EIPs return `PipelineOutcome` directly _(amends 0024)_
- [0026](./docs/adr/0026-json-canonical-route-authoring-format.md) — JSON is the canonical full-DSL authoring format for SDKs/generators; YAML is human convenience
- [0027](./docs/adr/0027-mqtt-component-3-1-1-per-endpoint-connections.md) — MQTT component uses MQTT 3.1.1 (v1) via `rumqttc`; one connection per Consumer/Producer created lazily (route_id not available at `create_endpoint()`); MQTT 5.0 deferred to v2
- [0028](./docs/adr/0028-claimcheck-repository-trait.md) — Separate `ClaimCheckRepository` trait (payload-bearing `set`/`get` with `Message` values; filter option for selective merge-back), distinct from key-only `IdempotentRepository` (ADR-0023); shared `NamedRegistry<T>` wiring pattern cross-referenced, not inherited

## Key Terms

Cross-cutting domain terms used across multiple crates. For crate-specific terms, see each crate's CONTEXT.md.

- **Message** — body+headers container inside an Exchange (`exchange.input`, `exchange.output`). Not the same as Exchange.
- **ErrorHandler / ErrorHandlerConfig / ExceptionPolicy** — DSL declares `ErrorHandler` and `OnException`; runtime compiles them into `ErrorHandlerConfig` and `ExceptionPolicy`. (camel-dsl + camel-core)
- **CircuitBreaker** — DSL-declared fault tolerance pattern. Not a Pipeline Step — compiles into error-handling middleware. (camel-dsl)
- **Supervision / ConsumerRestart** — Route-level crash recovery. Consumer task failure sends CrashNotification; RuntimeBus records route as Failed; optional restart policy recreates the whole Route with backoff. Consumers must not self-supervise. (camel-core)
- **ForcedHealthFailure / HealthCheckRegistry** — When a Consumer crashes (stop() never called), HealthCheckRegistry pins the route's entry to `Unhealthy` via `force_unhealthy_for_route()` until ConsumerRestart replaces it with a live probe. (services)
- **Degraded / Unhealthy** — Semantic rule for health and readiness: `Degraded` = HTTP 200 on /readyz, pod Ready (component can still process Exchanges). `Unhealthy` = HTTP 503, pod NotReady. Both `Healthy` and `Degraded` map to Ready; only `Unhealthy` maps to NotReady.
- **SecurityPolicy / Principal / AuthorizationDecision** — DSL declares route-level `security_policy`; Runtime wraps the Route Pipeline with `SecurityPolicyLayer` before normal Route Steps run. Grants store Principal properties on the Exchange; denials return `Unauthorized` into route error handling. (camel-dsl + camel-core + services/camel-auth + components/camel-component-wasm)
- **CanonicalRouteSpec / RouteDefinition** — `CanonicalRouteSpec` is the versioned stable minimal contract used by runtime commands, config tooling, and hot-reload paths. v2 adds lifecycle metadata (`auto_startup`, `startup_order`, `concurrency`). Unsupported fields are strictly rejected (no silent loss). `RouteDefinition` remains the full DSL route model. (camel-api + camel-dsl + camel-core, ADR-0011, ADR-0016)
- **Handler-contract boundary** — Conceptual line between an error emitter and the route element contractually responsible for owning that failure's log/metric/alerting. Emitters *inside* (taxonomy categories (a), (b-bridged)) MUST log at `warn!` or below; emitters *outside* (b′, g, e, c, d, f, h) own the operational signal and MAY log at `error!`. Established ADR-0012.
- **System-broken error** — Failure indicating corruption, panic-equivalent, or contract violation (taxonomy c, d, f, h). Always `error!`, never downgraded, no metric replacement required — the failure itself is the signal.
- **Side-effect failure** — Consumer failure that occurs AFTER a successful `ConsumerContext::send_and_wait` (taxonomy b′), e.g. SQL `onConsume` post-processing. The pipeline has already accepted the Exchange; no route-level handler will run for the side-effect failure. Emitter owns the signal; downgrade requires `MetricsCollector::increment_errors(route_id, "b-prime:<component>:<site>")`.
- **Bridged error** — Consumer failure converted to a synthetic error-bearing Exchange via `ConsumerContext::send_and_wait` (taxonomy b-bridged). Route's error handler owns the operational signal; the consumer's `bridge_*` path MUST log at `warn!` or below to avoid duplicate `error!`. Canonical implementation: `crates/components/camel-sql/src/consumer.rs:280-288`.
- **PollingConsumer** — Pull-based adapter created on demand from an Endpoint, delivering one Exchange per call. Contrasts with the event-driven Consumer (push model) that drives a Route. Endpoints opt in via `Endpoint::polling_consumer`; components that are purely event-driven (HTTP server, Kafka) return `None` by default. Used by the EIP-7 `pollEnrich` DSL verb and the WASM `camel_poll` host function. Established by ADR-0015. (camel-component-api + camel-processor + camel-core)
- **EnrichmentStrategy** — Strategy that merges the original Exchange with the enriched/polled Exchange in the EIP-7 `enrich` and `pollEnrich` verbs. Distinct from the EIP-22 `AggregateStrategyDef` family (which collides on the obvious name "AggregationStrategy"). Established by ADR-0015. (camel-processor + camel-dsl)
- **Starting Route** — Externally observable Route lifecycle state recorded after start intent is accepted and before the Consumer/Pipeline side effect is confirmed. Operators may see `Starting` in `RouteStatusProjection`; it is not an internal-only transient. Established by ADR-0018. (camel-core)
- **StopSegment**:
  Outcome-aware analog of `CompiledStep::Stop` for use inside structural EIP sub-pipelines. Always returns `PipelineOutcome::Stopped(ex)`.
  _Avoid_: stop processor, stop service (use StopSegment for the struct).

- **RetryableStep**:
  Object-safe trait unifying `BoxProcessor` and `OutcomeSegment` for `RouteErrorHandler::retry_step`. Single retry path serves both Tower processors and outcome-aware segments.
  _Avoid_: retry adapter, retry handler (use RetryableStep for the trait).

- **Route lifecycle compensation** — Control-plane recovery rule: if a lifecycle side effect fails after durable intent/state changed, Runtime records the Route as `Failed`, reconciles the status projection, and publishes failure events instead of rolling history back. Established by ADR-0018. (camel-core)
- **ExceptionDisposition** — Enum (`Propagate | Handled | Continued`) that replaces `handled: bool`. `Propagate` returns the error upstream; `Handled` absorbs and terminates the route normally; `Continued` clears the error and advances to the next pipeline step. Established by ADR-0019. (camel-api + camel-processor + camel-dsl)
- **RouteErrorHandler** — Trait injected into the pipeline with 4 async methods (`match_policy`, `retry_step`, `handle_step`, `handle_boundary`). The pipeline calls these after each step failure; the returned disposition drives the loop. `DefaultRouteErrorHandler` is the production implementation. Established by ADR-0019. (camel-processor + camel-core)
- **RouteChannelService** — Service that chains Security → CircuitBreaker(`before_call`) → Pipeline(`run_steps`) → CircuitBreaker(`after_result`). Constructed only when an `errorHandler` is configured. Boundary errors from Security or CB gates go through `handle_boundary`. Established by ADR-0019. (camel-core)
- **LlmProvider** — Trait abstraction over LLM backends (OpenAI, Ollama, Mock). Camel-shaped, not siumai-shaped. Owned by `LlmComponent` as `Arc<dyn LlmProvider>` in a `ProviderMap`. All siumai imports confined to the adapter. Established by ADR-0020. (camel-component-llm)
- **ProviderMap** — `HashMap<String, Arc<dyn LlmProvider>>` owned by `LlmComponent`. Resolved by name from config. Not a global registry — safe for tests, hot-reload, multi-context. Established by ADR-0020. (camel-component-llm)
- **OutcomePipeline**:
  Internal trait one layer above Tower for structural EIP sub-pipelines. Returns `PipelineOutcome` directly (NOT Tower `Result<Exchange, CamelError>`), so `Stopped(ex)` propagates with Exchange state intact. Implementations: FilterSegment, ChoiceSegment, LoopSegment, ThrottleSegment, DoTrySegment, SplitSegment, StreamingSplitSegment, MulticastSegment, LoadBalanceSegment.
  _Avoid_: outcome service, segment processor (use OutcomePipeline for trait, Segment for the CompiledStep variant).

- **OutcomeSegment**:
  Wrapper struct over `Box<dyn OutcomePipeline>` with extension hooks for tracing/metrics. Lives in camel-api (`crates/camel-api/src/outcome_segment.rs`); composition helpers (`SequentialOutcomeSegment`, `BoxProcessorSegment`, `StopSegment`, `BodyCoercingSegment`, `compose_outcome_segment`) stay in camel-core. Used as the payload of `CompiledStep::Segment`.
  _Avoid_: outcome box, pipeline wrapper.

- **PipelineOutcome** — Enum (`Completed(Exchange) | Stopped(Exchange) | Failed(CamelError)`) produced by `run_steps` (the pipeline executor). Lives ONE LAYER ABOVE Tower — `BoxProcessor::Response` and every `Service<Exchange>::Response` stays `Result<Exchange, CamelError>`. The pipeline's `Service<Exchange>` impl translates `PipelineOutcome` to `Result` via `into_tower_result()` (Completed/Stopped both → Ok). Stop EIP is successful control flow, not an error. Established by ADR-0024. (camel-api + camel-core + camel-processor)
- **ConsumerStopping** — `CamelError` variant for producer `poll_ready` shutdown signals. Distinct from Stop EIP — it indicates the producer's semaphore/channel is closing and the call cannot proceed. Used by JMS/OpenSearch producers. Established by ADR-0024. (camel-api + camel-component-jms + camel-component-opensearch)

## Documentation Authority & Refresh

Prose docs can drift from code. To keep them trustworthy, the project uses a fixed authority order
and an event-driven refresh rule.

**Authority order (highest wins on conflict):**

1. **Source code** — the only ground truth.
2. **`docs/ARCHITECT.md`** — code-derived snapshot pinned to a git SHA. Authoritative over all
   other prose, but may itself lag HEAD (see its header).
3. **`CONTEXT-MAP.md` + crate `CONTEXT.md`** — curated domain language and bounded-context map.
4. **`README.md` files** — user-facing summaries; most drift-prone (enum tables, Cargo metadata).

**Term-landing rule:** a new cross-cutting domain term lands in `CONTEXT-MAP.md` Key Terms first;
a crate-local term lands in that crate's `CONTEXT.md`. The defining ADR (if any) is cited inline.

**Refresh is event-driven, not scheduled:**

- After an **architecture-shaping merge** (new EIP, lifecycle change, contract change): regenerate
  `docs/ARCHITECT.md` from code and re-run the drift cross-check (`docs/ARCHITECT-DRIFT-REPORT.md`).
- When **adding/renaming a domain term**: update CONTEXT-MAP/CONTEXT in the same change.
- When an **ADR is superseded or amended**: update both the ADR header metadata and this map's ADR
  index in the same change.

`AGENTS.md` already requires reading `CONTEXT-MAP.md` before implementation, which is the
enforcement hook for the term-landing rule.

### CONTEXT.md coverage policy

Not every crate needs a crate-local `CONTEXT.md`. Coverage is role-based:

- **Must have one** — public contract crates (`camel-api`, `camel-component-api`,
  `camel-language-api`), runtime/control-plane crates with behavioral invariants (`camel-core`),
  and crates that are user-visible, stateful, security-sensitive, or operationally surprising
  (e.g. `camel-auth`, leader election, SEDA fanout, control bus, health/platform).
- **May defer to a parent `CONTEXT.md`** — thin adapters covered by `components/CONTEXT.md`; leaf
  language implementations with no distinct value/coercion/security semantics beyond
  `languages/CONTEXT.md`; examples (parent `examples/CONTEXT.md`).
- **Usually none needed** — macro helpers, bench/test harnesses, generated protobuf/WIT plumbing,
  and private build tooling, unless they expose user-facing vocabulary or architecture contracts.
