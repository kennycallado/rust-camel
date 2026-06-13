# Context Map

## Contexts

- [Runtime](./crates/camel-core/CONTEXT.md) — core execution engine: Exchange lifecycle, Route management, and component/language/function/service registries
- [DSL](./crates/camel-dsl/CONTEXT.md) — route definition via fluent builder API and YAML/JSON configuration files
- [Components](./crates/components/CONTEXT.md) — inbound/outbound adapters (timer, HTTP, Kafka, file, etc.) that feed Exchanges into Routes and send them to external systems
  - [LLM Component](./crates/components/camel-component-llm/CONTEXT.md) — LLM chat and embedding component (OpenAI, Ollama, Mock) with streaming and materialized modes
- [Languages](./crates/languages/CONTEXT.md) — expression and predicate evaluation against Exchanges (JavaScript, JSONPath, XPath, Simple, Rhai)
- [Functions](./crates/services/camel-function/CONTEXT.md) — out-of-process executable units invoked as pipeline steps; inspired by serverless functions, running in isolated containers
- [Services](./crates/services/CONTEXT.md) — cross-cutting infrastructure services: observability (OTel, Prometheus), auth/security, and platform integration (Kubernetes)

## Relationships

- **DSL → Runtime**: DSL compiles route definitions into `RouteDefinition` objects consumed by `CamelContext`
- **Components → Runtime**: Components register by URI scheme into `CamelContext`; Consumers submit `ExchangeEnvelope` into Route pipelines
- **Runtime → Components**: Runtime resolves a Component by scheme, creates an Endpoint, and starts a Consumer for each Route's `from:` URI
- **Languages → Runtime**: Language implementations register into `CamelContext`; the runtime resolves them to evaluate expressions and predicates within Pipeline steps
- **Functions → Runtime**: A `FunctionInvoker` is registered in `CamelContext`; the `function:` Pipeline step calls it with an Exchange and applies the returned patch
- **Services → Runtime**: Services implement `Lifecycle`, `MetricsCollector`, or `PlatformService` contracts and register into `CamelContext` for coordinated start/stop

## Architecture Decisions

Cross-cutting decisions that shaped the architecture live in [`docs/adr/`](./docs/adr/):

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
- [0016](./docs/adr/0016-canonical-route-spec-v2-contract.md) — CanonicalRouteSpec v2 adds lifecycle/execution metadata with strict rejection for unsupported fields
- [0017](./docs/adr/0017-dsl-yaml-snake-case-naming-convention.md) — DSL YAML keys use snake_case to match Rust field names and schema output
- [0018](./docs/adr/0018-two-phase-route-lifecycle-persistence.md) — Route lifecycle commands persist intent before side effects, use optimistic versions, and compensate to Failed on side-effect failure
- [0019](./docs/adr/0019-error-disposition-pipeline-recovery.md) — Error disposition decisions moved inside the pipeline loop via RouteErrorHandler trait injection; ExceptionDisposition enum (Propagate/Handled/Continued) replaces handled:bool
- [0020](./docs/adr/0020-llm-component-provider-adapter-boundary.md) — LLM component isolates siumai SDK behind a project-owned LlmProvider trait; all siumai imports confined to two files

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
- **Route lifecycle compensation** — Control-plane recovery rule: if a lifecycle side effect fails after durable intent/state changed, Runtime records the Route as `Failed`, reconciles the status projection, and publishes failure events instead of rolling history back. Established by ADR-0018. (camel-core)
- **ExceptionDisposition** — Enum (`Propagate | Handled | Continued`) that replaces `handled: bool`. `Propagate` returns the error upstream; `Handled` absorbs and terminates the route normally; `Continued` clears the error and advances to the next pipeline step. Established by ADR-0019. (camel-api + camel-processor + camel-dsl)
- **RouteErrorHandler** — Trait injected into the pipeline with 4 async methods (`match_policy`, `retry_step`, `handle_step`, `handle_boundary`). The pipeline calls these after each step failure; the returned disposition drives the loop. `DefaultRouteErrorHandler` is the production implementation. Established by ADR-0019. (camel-processor + camel-core)
- **RouteChannelService** — Service that chains Security → CircuitBreaker(`before_call`) → Pipeline(`run_steps`) → CircuitBreaker(`after_result`). Constructed only when an `errorHandler` is configured. Boundary errors from Security or CB gates go through `handle_boundary`. Established by ADR-0019. (camel-core)
- **LlmProvider** — Trait abstraction over LLM backends (OpenAI, Ollama, Mock). Camel-shaped, not siumai-shaped. Owned by `LlmComponent` as `Arc<dyn LlmProvider>` in a `ProviderMap`. All siumai imports confined to the adapter. Established by ADR-0020. (camel-component-llm)
- **ProviderMap** — `HashMap<String, Arc<dyn LlmProvider>>` owned by `LlmComponent`. Resolved by name from config. Not a global registry — safe for tests, hot-reload, multi-context. Established by ADR-0020. (camel-component-llm)
