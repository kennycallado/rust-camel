# Architecture

The system-level architecture: the plane split, route lifecycle, shutdown and
backpressure, and the crate dependency graph. The [Core concepts](../concepts/index.md) section holds the mental model. This page
builds on it and indexes the decisions that shaped it.

## Data plane vs control plane

rust-camel separates message flow from lifecycle. The data plane is the hot
path. Every Exchange flows through a Tower `Service<Exchange>` pipeline. The
control plane is the cold path. It owns route lifecycle through a CQRS
RuntimeBus with optimistic versioning.

The [Data plane vs control plane](../concepts/planes.md) concept page covers the
rationale, the performance and safety goals, and the Exchange trust boundary.
ADR-0001 records the decision to adopt Tower as the data-plane foundation.

## Route lifecycle

Route lifecycle follows a two-phase persist-then-execute pattern (ADR-0018).
The Runtime records intent before side effects, then confirms or compensates
after the side effect returns. For `StartRoute`, the sequence is:

1. Record `RouteStartRequested`, project Route as `Starting`
2. Start the Consumer and Pipeline
3. Record `RouteStarted`, project `Started`

If the side effect fails after intent was recorded, the Runtime records
`RouteFailed`, projects `Failed`, and publishes failure events. Compensation
applies to every non-atomic lifecycle flow (ADR-0018).

The lifecycle layer uses hexagonal architecture (ADR-0003). Domain and
application logic sit behind ports (`RouteRepositoryPort`,
`ProjectionStorePort`, `RuntimeEventJournalPort`, `RuntimeExecutionPort`).
Concrete adapters provide in-memory and redb implementations. ADR-0045 extends
this discipline crate-wide. Every behavioral area in `camel-core` is a vertical
slice with its own `domain` / `application` / `ports` / `adapters` layout.

Stateful pipeline steps (aggregators, resequencers, idempotent repositories)
implement the `StepLifecycle` trait (ADR-0022). The trait adds a drain hook
that outlives a single `process()` call. The drain lets background work
(timers, buckets, queues) complete before the step shuts down.

## Shutdown and backpressure

A route stop signals in-flight pipelines through a `tokio::task_local!` cancel
token (ADR-0043). The step loop checks the token before each step and returns
`Failed(ConsumerStopping)` on cancel. This gives cooperative cancellation
without interrupting a step mid-`.await`. The
[Data plane vs control plane](../concepts/planes.md) page covers the token
mechanics and the graceful-drain ordering.

Route-admission backpressure (ADR-0044) caps concurrent in-flight Exchanges.
The Concurrent consumer model acquires a semaphore permit before it dequeues
work. When permits run out, the consumer blocks on the permit instead of
buffering more work inside the pipeline task. This bounds memory under load.

## Crate dependency overview

The crate dependency graph follows a layered structure. Contract crates sit at
the bottom with zero or minimal internal dependencies. Runtime and processing
crates depend on contracts. Components, languages, and services depend on the
runtime and contracts. Platforms depend on services.

```text
Platforms
    |
Services
    |
Components ----> Runtime ----> Processors
    |               |              |
    |               v              v
    +---------> Contracts <--------+
                    ^
                    |
              Languages
```

**Contracts** (`camel-api`, `camel-component-api`, `camel-language-api`,
`camel-wit`, `camel-config`, `camel-endpoint`, `camel-bean`, `camel-test`,
`camel-bench`) define the types and traits that every other crate depends on.
They have no runtime dependency.

**Runtime** (`camel-core`, `camel-cli`, `camel-health`) owns the execution
engine, route lifecycle, hot reload, and registries. `camel-core` depends on
contract crates and drives every other family.

**Processors** (`camel-processor`, `camel-builder`) implement EIP patterns as
Tower middleware. They depend on `camel-api` for `Exchange` and `Processor`.

**DSL** (`camel-dsl`) parses YAML and JSON route definitions into
`RouteDefinition`. It depends on `camel-api` and `camel-builder`.

**Components** connect routes to external systems. Each component crate depends
on `camel-component-api` and `camel-core`. See the
[component catalog](../components/index.md) for the full list.

**Languages** evaluate expressions and predicates. Each language crate depends
on `camel-language-api`. See the [language catalog](../languages/index.md) for
the full list.

**Services** provide cross-cutting infrastructure: auth, observability, and
function invocation. They register into `CamelContext` through the
[service contracts](https://github.com/kennycallado/rust-camel/blob/main/crates/services/CONTEXT.md). See the
[service catalog](../services/index.md) for the full list.

**Platforms** expose deployment-aware behaviour. See the
[platform catalog](../platforms/index.md) for the full list.

**Data formats** convert between wire representations and structured body
types. See the [data format catalog](../data-formats/index.md) for the full
list.

For the complete bounded-context map and domain vocabulary, see
[CONTEXT-MAP.md](https://github.com/kennycallado/rust-camel/blob/main/CONTEXT-MAP.md).

## ADR index

Architecture-shaping choices live as ADRs under
[`../adr/`](../adr/). The index below organizes them by topic. Each
entry links to the ADR file and gives a one-sentence summary.

See also [Important Findings Summary](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/IMPORTANT-FINDINGS-SUMMARY.md)
for resolved P0 findings.

### Architecture and Design

Core patterns, lifecycle, pipeline, and route authoring.

| ADR | Title | Summary |
| --- | --- | --- |
| [0001](../adr/0001-tower-data-plane-split-from-control-plane.md) | Tower data plane, custom-trait control plane | Separates Exchange processing (Tower `Service<Exchange>`) from component lifecycle (custom traits). |
| [0002](../adr/0002-cqrs-runtime-bus-for-route-lifecycle.md) | CQRS RuntimeBus for route lifecycle | Route lifecycle mutations go through `RuntimeCommandBus` with projections and optional event journal. |
| [0003](../adr/0003-hexagonal-lifecycle-core.md) | Hexagonal lifecycle core | Lifecycle layer uses ports and adapters for persistence and testability. |
| [0004](../adr/0004-hot-reload-atomic-pipeline-swap.md) | Hot reload via atomic pipeline swap | Pipeline swap uses `ArcSwap` so in-flight Exchanges complete against the snapshot they entered. |
| [0005](../adr/0005-function-out-of-process-staged-reload.md) | Function out-of-process staged reload | `function:` steps run in isolated containers with prepare/finalize/discard registration. |
| [0006](../adr/0006-script-synchronous-boa-async-to-function.md) | Script synchronous, async to function | JavaScript evaluation is synchronous; async paths delegate to `function:`. |
| [0007](../adr/0007-route-supervised-consumer-failure.md) | Route-supervised consumer failure | Consumer task failure is route-supervised with optional restart policy and backoff. |
| [0008](../adr/0008-route-templates-json-tree-substitution.md) | Route templates via JSON tree substitution | Template placeholders expand via JSON tree walk before DSL deserialization. |
| [0009](../adr/0009-http-co-hosting-api-and-static-routes.md) | HTTP co-hosting API and static routes | API routes and static mounts share one server per host/port with deterministic dispatch. |
| [0011](../adr/0011-canonical-route-spec-minimal-contract.md) | CanonicalRouteSpec minimal contract | v1 is a stable minimal route contract, not a full `RouteDefinition` mirror. |
| [0015](../adr/0015-endpoint-created-polling-consumer-for-pollenrich.md) | Endpoint-created PollingConsumer | Pull-based adapter created from an Endpoint for `pollEnrich` and WASM `camel_poll`. |
| [0016](../adr/0016-canonical-route-spec-v2-contract.md) | CanonicalRouteSpec v2 contract | v2 adds lifecycle metadata with strict rejection for unsupported fields. |
| [0017](../adr/0017-dsl-yaml-snake-case-naming-convention.md) | DSL YAML snake_case naming | DSL keys use snake_case to match Rust field names and schema output. |
| [0018](../adr/0018-two-phase-route-lifecycle-persistence.md) | Two-phase route lifecycle persistence | Lifecycle commands persist intent before side effects, compensate on failure. |
| [0022](../adr/0022-steplifecycle-trait-and-drain.md) | StepLifecycle trait and drain | Stateful pipeline steps get a separate drain hook for background work. |
| [0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md) | PipelineOutcome replaces CamelError::Stopped | `PipelineOutcome` enum replaces `CamelError::Stopped` for pipeline control flow. |
| [0025](../adr/0025-outcome-aware-structural-eips.md) | Outcome-aware structural EIPs | Structural EIPs return `PipelineOutcome` directly instead of Tower `Result`. |
| [0026](../adr/0026-json-canonical-route-authoring-format.md) | JSON canonical route authoring | JSON is the canonical full-DSL format for SDKs and generators; YAML is human convenience. |
| [0029](../adr/0029-resequencer-continuation-boundary.md) | Resequencer continuation boundary | Compiler splits step list at `Resequence`; post-steps compile into a continuation owned by the service. |
| [0030](../adr/0030-exchange-aware-dataformat-hooks.md) | Exchange-aware DataFormat hooks | `DataFormat` gains default `marshal_in_exchange` / `unmarshal_in_exchange` hooks. |
| [0031](../adr/0031-wasm-source-world.md) | WASM source world | Fourth WIT world `source` lets WASM guests act as Consumers with their own consumption loop. |
| [0041](../adr/0041-component-metadata-capabilities-schema.md) | Component metadata capabilities schema | `ComponentMetadata` schema with `OptionKind`, `UriOption`, `ComponentCapabilities`, `CapabilityQuery`. |
| [0042](../adr/0042-arc-compiled-steps-snapshot.md) | Arc\<[CompiledStep]\> shared snapshot | Shared snapshot avoids per-Exchange Vec clone for compiled pipeline steps. |
| [0043](../adr/0043-pipeline-cancellation-between-steps.md) | Pipeline cancellation between steps | `task_local!` cancel token checked between steps for cooperative cancellation. |
| [0044](../adr/0044-route-admission-backpressure.md) | Route-admission backpressure | Semaphore permit acquired before dequeue prevents unbounded in-flight work. |
| [0045](../adr/0045-camel-core-architecture-charter.md) | camel-core architecture charter | Codifies Clean + DDD + CQRS + vertical slices + hexagonal discipline crate-wide. |
| [0046](../adr/0046-apache-camel-inspiration-not-conformance.md) | Apache Camel inspiration, not conformance | Apache Camel is design inspiration, not conformance authority. |
| [0047](../adr/0047-template-rendering-engine.md) | Template rendering engine | MiniJinja-based external template engine with compile-once caching and atomic hot reload. |
| [0053](../adr/0053-wit-interface-versioning.md) | WIT interface versioning | `camel:plugin` uses one package-level WIT SemVer, independent from Rust crate versions. |

### Security

Authentication, authorization, trust boundaries, and capability models.

| ADR | Title | Summary |
| --- | --- | --- |
| [0010](../adr/0010-security-policy-pre-pipeline-authorization.md) | SecurityPolicy pre-pipeline authorization | Route-level authorization wraps the Pipeline before any step runs. |
| [0032](../adr/0032-exchange-data-trust-boundary.md) | Exchange-data trust boundary | Operator config is trusted; exchange data is untrusted and must not drive control-plane actions. |
| [0033](../adr/0033-security-defaults-fail-closed-startup-validation.md) | Security defaults and fail-closed startup validation | Five-disposition security policy enforced by a single startup-validation phase. |
| [0034](../adr/0034-controlbus-capability-authz.md) | ControlBus capability authorization | ControlBus requires an `authorizedRoutes` allowlist and denies self-restart. |
| [0035](../adr/0035-leader-epoch-fencing-token.md) | Leader-epoch fencing token | Every `master:` delegate envelope carries a monotonic fencing token for split-brain safety. |
| [0036](../adr/0036-bridge-ipc-mtls.md) | Bridge IPC mutual TLS | Bridge uses mutual TLS with ephemeral certificates; fail-closed guard rejects placeholder paths. |
| [0037](../adr/0037-exec-component-fail-closed-capability-model.md) | Exec component fail-closed capability model | Allowlisted binaries, argument policy, no shell, bounded stdin. |
| [0050](../adr/0050-wasm-sandbox-capability-posture.md) | WASM sandbox capability posture | Per-world grants for Camel host functions and selective WASI registration. |
| [0051](../adr/0051-credential-redaction-at-diagnostic-boundaries.md) | Credential redaction at diagnostic boundaries | Credential-bearing types use manual redacting `Debug`; `Serialize` must not expose credential bytes. |
| [0052](../adr/0052-diagnostic-endpoint-exposure-posture.md) | Diagnostic endpoint exposure posture | Diagnostic endpoints follow the Prometheus scrape model; network isolation is the operator's duty. |

### Error Handling

Disposition, drain, supervision, and repository patterns.

| ADR | Title | Summary |
| --- | --- | --- |
| [0012](../adr/0012-log-level-convention-handler-contract-boundaries.md) | Log-level convention by handler-contract boundaries | Emitters inside a handler contract log at `warn!` or below; outside emitters may log at `error!`. |
| [0019](../adr/0019-error-disposition-pipeline-recovery.md) | Error disposition in-pipeline recovery | `RouteErrorHandler` trait injected into the pipeline decides disposition after each step failure. |
| [0023](../adr/0023-idempotent-repository-trait.md) | Idempotent Repository trait | Key-only `IdempotentRepository` trait in `camel-api` for duplicate detection. |
| [0028](../adr/0028-claimcheck-repository-trait.md) | Claim Check Repository trait | Payload-bearing `ClaimCheckRepository` trait distinct from key-only `IdempotentRepository`. |

### Performance and Limits

DoS caps, cardinality limits, and resource bounds.

| ADR | Title | Summary |
| --- | --- | --- |
| [0038](../adr/0038-configurable-dos-caps-via-per-format-config-channel.md) | Configurable DoS caps | Per-format config channel for operator-overridable data-format DoS caps. |
| [0039](../adr/0039-configurable-loop-iteration-cap.md) | Configurable loop iteration cap | Per-step `max_iterations` escape hatch for loop iteration limits. |
| [0040](../adr/0040-configurable-materialize-limits.md) | Configurable materialize limits | Configurable materialize limits for XSLT, XJ, and WASM producers. |

### Integration

WASM, functions, components, and cross-cutting contracts.

| ADR | Title | Summary |
| --- | --- | --- |
| [0013](../adr/0013-network-retry-policy-and-migration.md) | NetworkRetryPolicy and migration | Centralized retry semantics and migration boundaries for adapter retries. |
| [0014](../adr/0014-wasm-plugin-config-unification.md) | WASM plugin config unification | Unified WASM plugin runtime configuration across all plugin types. |
| [0020](../adr/0020-llm-component-provider-adapter-boundary.md) | LLM component provider adapter boundary | LLM component isolates SDK behind a project-owned `LlmProvider` trait. |
| [0021](../adr/0021-llm-retry-retry-after-manual-loop.md) | LLM retry with retry-after manual loop | LLM retry honors provider `retry_after` via manual loop, diverging from ADR-0013. |
| [0027](../adr/0027-mqtt-component-3-1-1-per-endpoint-connections.md) | MQTT component 3.1.1 per-endpoint | MQTT 3.1.1 via `rumqttc` with one connection per Consumer or Producer. |
| [0048](../adr/0048-attestation-provenance-retired.md) | Attestation provenance (retired) | Retired HMAC-SHA256 attestation decision kept for history. |
| [0049](../adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md) | Workspace non-exhaustive policy | Public contract enums are `#[non_exhaustive]` by default before the 1.0 API freeze. |
