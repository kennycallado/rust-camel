# Context Map

## Contexts

- [Runtime](./crates/camel-core/CONTEXT.md) — core execution engine: Exchange lifecycle, Route management, and component/language/function/service registries
- [DSL](./crates/camel-dsl/CONTEXT.md) — route definition via fluent builder API and YAML/JSON configuration files
- [Components](./crates/components/CONTEXT.md) — inbound/outbound adapters (timer, HTTP, Kafka, file, etc.) that feed Exchanges into Routes and send them to external systems
- [Languages](./crates/languages/CONTEXT.md) — expression and predicate evaluation against Exchanges (JavaScript, JSONPath, XPath, Simple, Rhai)
- [Functions](./crates/services/camel-function/CONTEXT.md) — out-of-process executable units invoked as pipeline steps; inspired by serverless functions, running in isolated containers
- [Services](./crates/services/CONTEXT.md) — cross-cutting infrastructure services: observability (OTel, Prometheus) and platform integration (Kubernetes)

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

## Key Terms

Cross-cutting domain terms used across multiple crates. For crate-specific terms, see each crate's CONTEXT.md.

- **Message** — body+headers container inside an Exchange (`exchange.input`, `exchange.output`). Not the same as Exchange.
- **ErrorHandler / ErrorHandlerConfig / ExceptionPolicy** — DSL declares `ErrorHandler` and `OnException`; runtime compiles them into `ErrorHandlerConfig` and `ExceptionPolicy`. (camel-dsl + camel-core)
- **CircuitBreaker** — DSL-declared fault tolerance pattern. Not a Pipeline Step — compiles into error-handling middleware. (camel-dsl)
- **Supervision / ConsumerRestart** — Route-level crash recovery. Consumer task failure sends CrashNotification; RuntimeBus records route as Failed; optional restart policy recreates the whole Route with backoff. Consumers must not self-supervise. (camel-core)
- **ForcedHealthFailure / HealthCheckRegistry** — When a Consumer crashes (stop() never called), HealthCheckRegistry pins the route's entry to `Unhealthy` via `force_unhealthy_for_route()` until ConsumerRestart replaces it with a live probe. (services)
- **Degraded / Unhealthy** — Semantic rule for health and readiness: `Degraded` = HTTP 200 on /readyz, pod Ready (component can still process Exchanges). `Unhealthy` = HTTP 503, pod NotReady. Both `Healthy` and `Degraded` map to Ready; only `Unhealthy` maps to NotReady.
