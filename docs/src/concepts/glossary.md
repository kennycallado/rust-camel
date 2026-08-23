# Glossary

Glossary of names used across this guide. Bold entries are cross-cutting
domain terms registered in `CONTEXT-MAP.md`. Plain-text entries are
foundational primitives defined in their owning crate's `CONTEXT.md`. The
bold list is alphabetical. Each entry links its canonical guide page and
the decision or crate that defines it.

## Cross-cutting terms

- **ArcSwap\<TlsAcceptor\>** — atomic-swap holder for the gRPC TLS acceptor.
  Each accept loop loads a cert snapshot. HTTP and WS swap certs through
  `RustlsConfig`. [Hot reload](../configuration/hot-reload.md),
  [ADR-0004](../adr/0004-hot-reload-atomic-pipeline-swap.md).
- **Bounded context** — behavioral area of camel-core that owns its own
  domain vocabulary. The CQRS flavor is a per-context decision, never a
  crate-wide one. [Architecture](../architecture/index.md),
  [ADR-0045](../adr/0045-camel-core-architecture-charter.md).
- **Bridged error** — consumer failure that a `bridge_*` path converts into a
  synthetic error-bearing Exchange through `send_and_wait`. The route error
  handler owns the operational signal. [Error handling](error-handling.md),
  [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md).
- **CanonicalRouteSpec** — versioned minimal route contract that runtime
  commands, config tooling, and hot-reload consume. v2 adds lifecycle
  metadata and rejects unsupported fields. [Route structure](../yaml-dsl/route-structure.md),
  ADR-0011, [ADR-0016](../adr/0016-canonical-route-spec-v2-contract.md).
- **CircuitBreaker** — DSL-declared fault tolerance pattern. It compiles into
  error-handling middleware, not a Pipeline Step. [Circuit breaker](../eip/circuit-breaker.md),
  [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).
- **ConsumerStopping** — `CamelError` variant for producer shutdown. It is
  raised in the producer `call()` when the channel or semaphore is closing.
  Distinct from Stop EIP. [Error handling](error-handling.md),
  [ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md).
- **Credential redaction boundary** — types that hold passwords, tokens,
  keys, or credential bytes must not expose those values through `Debug` or
  general-purpose `Serialize`. Use manual redaction or a tested wrapper.
  [Auth](../services/auth.md),
  [ADR-0051](../adr/0051-credential-redaction-at-diagnostic-boundaries.md).
- **Degraded** — health state meaning the component can still process
  Exchanges: HTTP 200 on `/readyz`, pod Ready. `Unhealthy` returns HTTP 503
  and marks the pod NotReady. [Health](../operations/health.md).
- **DivertCopyTo** — `InterceptAction` variant that copies the exchange to a
  `mock:` target with WireTap semantics and then runs the real producer.
  [Testing](../testing/index.md), [ADR-0064](../adr/0064-two-tier-testing-contract.md).
- **EnrichmentStrategy** — strategy that merges the original Exchange with
  the polled or enriched Exchange in the `enrich` and `pollEnrich` verbs.
  Distinct from the EIP-22 `AggregateStrategyDef` family.
  [Content enricher](../eip/content-enricher.md),
  [ADR-0015](../adr/0015-endpoint-created-polling-consumer-for-pollenrich.md).
- **ErrorHandler** — DSL declaration (`ErrorHandler`, `OnException`) that
  compiles into `ErrorHandlerConfig` and `ExceptionPolicy` at runtime.
  [Error handling](error-handling.md),
  [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).
- **Exchange-data trust boundary** — operator config is trusted. Exchange
  data (headers, body, properties, correlation keys) is untrusted and
  adversary-controlled. Such data must not reach a control-plane action, an
  unbounded decision, or an executable sink without validation.
  [Planes](planes.md), [ADR-0032](../adr/0032-exchange-data-trust-boundary.md).
- **ExceptionDisposition** — enum (`Propagate | Handled | Continued`) that
  replaces `handled: bool`. `Propagate` returns the error upstream. `Handled`
  ends the route normally. `Continued` clears the error and advances.
  [Error handling](error-handling.md),
  [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).
- **ForcedHealthFailure** — when a Consumer crashes, `HealthCheckRegistry`
  pins the route's health entry to `Unhealthy` through
  `force_unhealthy_for_route()` until a `ConsumerRestart` replaces it with a
  live probe. [Health](../operations/health.md).
- **Handler-contract boundary** — conceptual line between an error emitter
  and the route element that owns the failure's operational signal. Emitters
  inside the boundary log at `warn!` or below. [Error handling](error-handling.md),
  [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md).
- **InterceptRule** — exact-URI rule that maps a send URI to a `SkipTo` or
  `DivertCopyTo` action. [Testing](../testing/index.md),
  [ADR-0064](../adr/0064-two-tier-testing-contract.md).
- **LlmProvider** — trait abstraction over LLM backends (OpenAI, Ollama,
  Mock). Camel-shaped, not siumai-shaped. All siumai imports stay in the
  adapter. [Components](../components/index.md),
  [ADR-0020](../adr/0020-llm-component-provider-adapter-boundary.md).
- **Message** — body and headers container inside an Exchange.
  `exchange.input` is the incoming Message. `exchange.output` is the optional
  reply Message. [Exchange & Message](exchange-message.md).
- **Module-discipline ceiling** — camel-core 1.0 policy. Clean Architecture
  rings are enforced by module paths and boundary tests, not by crate
  isolation. A crate split stays a post-1.0 option. [Architecture](../architecture/index.md),
  [ADR-0045](../adr/0045-camel-core-architecture-charter.md).
- **OpenAPI code-first generation** — `rest:` AST compiled into an OpenAPI
  3.0.3 document via `camel openapi generate` or
  `camel_dsl::openapi::generate_openapi()`. [YAML DSL](../yaml-dsl/index.md).
- **OutcomePipeline** — internal trait one layer above Tower for structural
  EIP sub-pipelines. It returns `PipelineOutcome` directly so `Stopped(ex)`
  keeps Exchange state intact. [Error handling](error-handling.md),
  [ADR-0025](../adr/0025-outcome-aware-structural-eips.md).
- **OutcomeSegment** — wrapper struct over `Box<dyn OutcomePipeline>` with
  tracing and metrics hooks. It is the payload of `CompiledStep::Segment`.
  [Error handling](error-handling.md),
  [ADR-0025](../adr/0025-outcome-aware-structural-eips.md).
- **PipelineOutcome** — enum (`Completed(Exchange) | Stopped(Exchange) |
  Failed(CamelError)`) produced by the pipeline executor one layer above
  Tower. Stop EIP is successful control flow, not an error.
  [Error handling](error-handling.md),
  [ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md).
- **PollingConsumer** — pull-based adapter created on demand from an
  Endpoint. It delivers one Exchange per call. Used by `pollEnrich` and the
  WASM `camel_poll` host function. [Poll enrich](../eip/poll-enrich.md),
  [ADR-0015](../adr/0015-endpoint-created-polling-consumer-for-pollenrich.md).
- **ProviderMap** — `HashMap<String, Arc<dyn LlmProvider>>` owned by
  `LlmComponent` and resolved by name from config. Not a global registry.
  [Components](../components/index.md),
  [ADR-0020](../adr/0020-llm-component-provider-adapter-boundary.md).
- **REST DSL** — declarative `rest:` YAML/JSON blocks that lower to `http:`
  consumer routes with JSON binding, path templates, and optional schema
  validation. [YAML DSL](../yaml-dsl/index.md).
- **RetryableStep** — object-safe trait that unifies `BoxProcessor` and
  `OutcomeSegment` for `RouteErrorHandler::retry_step`. One retry path serves
  both Tower processors and outcome-aware segments. [Error handling](error-handling.md),
  [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).
- **Route lifecycle compensation** — control-plane recovery rule. If a
  lifecycle side effect fails after durable intent changed, the Runtime marks
  the Route `Failed`. It reconciles the projection and publishes failure
  events instead of rolling history back. [Routes & pipelines](routes-pipelines.md),
  [ADR-0018](../adr/0018-two-phase-route-lifecycle-persistence.md).
- **RouteChannelService** — service that chains Security, CircuitBreaker
  (`before_call`), Pipeline (`run_steps`), and CircuitBreaker
  (`after_result`). Built only when an `errorHandler` is configured.
  [Error handling](error-handling.md),
  [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).
- **RouteErrorHandler** — trait injected into the pipeline with four async
  methods (`match_policy`, `retry_step`, `handle_step`, `handle_boundary`).
  The returned disposition drives the loop. [Error handling](error-handling.md),
  [ADR-0019](../adr/0019-error-disposition-pipeline-recovery.md).
- **SecurityPolicy** — route-level authorization contract applied before
  normal Route Steps run. Denials return `Unauthorized` into route error
  handling. [Auth](../services/auth.md),
  [ADR-0010](../adr/0010-security-policy-pre-pipeline-authorization.md).
- **Security defaults & fail-closed startup validation** — five-disposition
  policy (Intent-Violation, Intent-Declaration, Require-Explicit-Choice,
  Safety-Primitive, Untrusted-Data-Validation) enforced by one fail-closed
  startup phase. Each hardened default has its own per-item flag.
  [Auth](../services/auth.md),
  [ADR-0033](../adr/0033-security-defaults-fail-closed-startup-validation.md).
- **ServerTlsSource** — shared cert-file source struct (`cert_path`,
  `key_path`, `client_ca_path`) used by the gRPC, HTTP, and WS server
  components for initial TLS setup and reload. [Hot reload](../configuration/hot-reload.md).
- **Side-effect failure** — consumer failure that occurs after a successful
  `send_and_wait`, for example SQL `onConsume` post-processing. No
  route-level handler runs for it. The emitter owns the signal.
  [Error handling](error-handling.md),
  [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md).
- **SkipTo** — `InterceptAction` variant that replaces the original send and
  routes the exchange to a `mock:` target. [Testing](../testing/index.md),
  [ADR-0064](../adr/0064-two-tier-testing-contract.md).
- **Starting Route** — externally observable Route lifecycle state between
  accepted start intent and confirmed Consumer or Pipeline side effect.
  Operators can see `Starting` in `RouteStatusProjection`.
  [Routes & pipelines](routes-pipelines.md),
  [ADR-0018](../adr/0018-two-phase-route-lifecycle-persistence.md).
- **StopSegment** — outcome-aware analog of `CompiledStep::Stop` for
  structural EIP sub-pipelines. It always returns
  `PipelineOutcome::Stopped(ex)`. [Stop](../steps/stop.md),
  [ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md).
- **Supervision** — route-level crash recovery. A Consumer task failure sends
  a `CrashNotification`. The RuntimeBus records the route as `Failed`. An
  optional restart policy recreates the whole Route with backoff.
  [Error handling](error-handling.md),
  [ADR-0007](../adr/0007-route-supervised-consumer-failure.md).
- **Synchronous-projection CQRS** — CQRS variant where the read-side
  projection updates inside the same optimistic-versioned UnitOfWork as the
  command. This gives strong read-model freshness with no projection lag.
  [Planes](planes.md), [ADR-0002](../adr/0002-cqrs-runtime-bus-for-route-lifecycle.md).
- **System-broken error** — failure that indicates corruption, a panic
  equivalent, or a contract violation. Always logged at `error!`, never
  downgraded. [Error handling](error-handling.md),
  [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md).
- **Template rendering language** — language SPI implementation that renders
  templates (HTML, JSON, prompts) against Exchange data. Phase 1 covers
  inline templates. Phase 2 adds external file loading and hot-reload.
  [MiniJinja](../languages/minijinja.md),
  [ADR-0047](../adr/0047-template-rendering-engine.md).
- **TLS cert hot-reload** — platform-wide inbound TLS certificate rotation
  via `RuntimeCommand::ReloadTlsCerts { scheme, host, port }`. The reload is
  idempotent and skips the journal. [Hot reload](../configuration/hot-reload.md),
  [ADR-0004](../adr/0004-hot-reload-atomic-pipeline-swap.md).
- **TlsReloadHandler** — trait that each TLS-terminating component implements
  (`matches(scheme, host, port)` plus `async reload()`). Components register
  it lazily in `TlsReloadRegistry::global()`. [Hot reload](../configuration/hot-reload.md).
- **Vertical slice** — unit of decomposition for camel-core. Each bounded
  context is a self-contained slice with its own
  `domain`/`application`/`ports`/`adapters` layout, not a shared technical
  layer. [Architecture](../architecture/index.md),
  [ADR-0045](../adr/0045-camel-core-architecture-charter.md).
- **WASM sandbox capability posture** — per-world grant model across Camel
  host functions and WASI interfaces. Camel calls use explicit scheme
  allowlists. WASI uses selective registration, not full-linker registration
  with runtime denial. [Extending](../extending/index.md),
  [ADR-0050](../adr/0050-wasm-sandbox-capability-posture.md).

## Foundational primitives

Crate-local building blocks. The owning crate's `CONTEXT.md` is the
canonical definition. These terms are intentionally not bold: the glossary
tracks Key Terms only.

- Component — factory for Endpoints, identified by a URI scheme, registered
  into `CamelContext`. [Components & endpoints](components-endpoints.md),
  [Components bounded context](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).
- Consumer — inbound adapter the Runtime starts for a Route's `from:`
  Endpoint. [Components & endpoints](components-endpoints.md),
  [Components bounded context](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).
- EIP — Enterprise Integration Pattern. A Processor implemented as Tower
  middleware in `camel-processor`. [EIP patterns](../eip/index.md),
  [Processor crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).
- Endpoint — communication point a Component creates from a URI.
  [Components & endpoints](components-endpoints.md),
  [Components bounded context](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).
- Exchange — data envelope carrying an input Message, an optional output
  Message, properties, error state, and an exchange pattern.
  [Exchange & Message](exchange-message.md),
  [API contracts](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/CONTEXT.md).
- Producer — outbound adapter created for a `to:` Endpoint. It returns a
  `BoxProcessor` that sends an Exchange. [Components & endpoints](components-endpoints.md),
  [Components bounded context](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).
- Processor — one processing unit in a Pipeline. The universal step contract
  over a Tower service. [EIP patterns](../eip/index.md),
  [API contracts](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/CONTEXT.md).
- Route — named pipeline of a source Endpoint and an ordered step sequence.
  [Routes & pipelines](routes-pipelines.md),
  [Runtime](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md).
