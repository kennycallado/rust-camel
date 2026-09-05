# Runtime

The execution engine for rust-camel. Owns Exchange lifecycle, Route management, and the registries that wire together Components, Languages, Functions, and Services.

> **Scope boundary.** This file is the **behavioral vocabulary** — what the runtime *does* with the
> contract types. The contract/type definitions themselves (Exchange, Message, Body, CamelError,
> Processor, BoxProcessor, PipelineOutcome, the CQRS bus traits, CanonicalRouteSpec) are defined in
> the API Contracts context, [`crates/camel-api/CONTEXT.md`](../camel-api/CONTEXT.md). Terms below
> describe runtime semantics and intentionally reference rather than re-define those types.

## Language

**Exchange**:
The unit of data flowing through a Route — an envelope carrying input/output Messages, properties,
extensions, exchange pattern, error state, and optional correlation/tracing context.
`exchange.input` is always present; `exchange.output` is typically set only for `InOut` exchanges.
_Avoid_: message (when referring to the whole Exchange), event, request

**Message**:
The body+headers container inside an Exchange. `exchange.input` is the incoming Message carrying
the payload from the Consumer; `exchange.output` is an optional response Message produced by a
Processor for `InOut` exchanges. Components construct the initial Message; Processors read and
write it through Exchange accessors.
_Avoid_: exchange, event, request, envelope

**ExchangePattern**:
Declares whether an Exchange expects a reply. `InOnly` = fire-and-forget; `InOut` = request-reply.
_Avoid_: MEP (use the enum variant names directly)

**Route**:
A named message-processing pipeline: a source endpoint that emits Exchanges and a sequence of steps that transform or route them.
_Avoid_: flow, pipeline (when referring to the Route as a whole)

**Suspended Route**:
Route lifecycle state where Consumer intake is stopped while the Pipeline and route channel remain alive. `resume` recreates the Consumer without rebuilding the whole Route.
_Avoid_: stopped route, paused pipeline

**Starting Route**:
Route lifecycle state where start intent has been accepted and projected, but the runtime Consumer/Pipeline side effect has not yet been confirmed. Operators can observe this state in Route status queries.
_Avoid_: transient start flag, hidden startup state

**Pipeline**:
The compiled assembly of Processors that processes an Exchange through a Route's data plane.
_Avoid_: chain, middleware stack

**Processor**:
A single processing unit in a Pipeline — either an EIP pattern (filter, choice, split, setBody) or any custom step that receives and returns an Exchange.
_Avoid_: handler, middleware, transformer

**CamelContext**:
The composition root of the runtime. Manages component, language, function, and service registries;
controls Route lifecycle. Exposes metadata accessors: `component_metadata(scheme)`,
`all_component_metadata()`, and `metadata_catalog()` for trait-object lookup. Established by
ADR-0041.
_Avoid_: container, application context, context (unqualified)

**RuntimeObservability**:
Narrow trait implemented by the runtime and injected into Component Endpoints at `create_consumer` / `create_producer` time. Provides `metrics()` (counter increments for categories b′/e), `health()` (forced-unhealthy signalling for category g), and `component_metrics()` (the `ComponentMetrics` facade carrying the components-lever snapshot; controller-wired contexts override it, defaults lever-off) so Component code can record failures and opt-in success-path telemetry without taking a hard dependency on metrics/health infrastructure. Established by ADR-0012 Phase A; facade per ADR-0066.
_Avoid_: metrics handle, observability service, runtime hook (use RuntimeObservability when describing the trait)

**UnitOfWorkConfig**:
Optional per-Route configuration that wraps the Pipeline with exchange lifecycle tracking. When present, fires completion or failure hooks (by producer URI) when an Exchange exits the Pipeline.
_Avoid_: transaction, scope, UnitOfWork

**RuntimeBus**:
CQRS façade for the **Route control plane only**. Commands mutate Route state (start, stop, add); queries read current status from projections. Uses synchronous-projection CQRS (projection updated within the same UnitOfWork as the command — no read-model lag). The data plane (Exchange/Pipeline processing) is explicitly NOT CQRS. Established by ADR-0002; two-phase persistence refined by ADR-0018; scope/ceiling framed by ADR-0045.
_Avoid_: event bus, message bus, command bus (unqualified)

**Route Lifecycle Projection**:
Read-side Route status record maintained from lifecycle aggregate state. It is the source for RuntimeBus route-status queries and may show intermediate states such as `Starting`.
_Avoid_: controller state, live route state

**Route Lifecycle Compensation**:
Control-plane recovery outcome recorded when a lifecycle side effect fails after intent or state was persisted. The Route is marked `Failed` rather than silently rolling back accepted history.
_Avoid_: rollback, undo

**Adopt-on-recovery**:
Registration policy that lets the first re-registration of a Route id adopt an aggregate reconstructed by journal replay at boot, instead of rejecting it as a duplicate. `recover_from_journal` flags every replayed Route id as adoptable; `take_recovered` drains that flag exactly once. In `handle_register` and `handle_register_internal`, an existing aggregate whose flag is still set is adopted — the runtime side effect rebuilds the in-memory Pipeline and the persist overwrites the stale aggregate with a fresh `Registered` baseline, so `auto_startup` drives the Route up as on first boot. Without this policy a durable `runtime_journal` turns every restart after the first into an `AlreadyExists` failure for every declarative Route. A same-session duplicate (flag absent or already drained) is still rejected as `AlreadyExists`.
_Avoid_: duplicate route, upsert route

**Journal register-checkpoint compaction**:
Durable `runtime_journal` compaction rule that bounds file growth under Adopt-on-recovery. `RedbRuntimeEventJournal::compact` drops, per route id, every event with a `seq` below that route's last `RouteRegistered` — replaying a `RouteRegistered` resets the aggregate to a fresh `Registered` v0, so all earlier history is redundant. It runs together with the removed-route rule (drop everything up to the last `RouteRemoved`). Without it, a declarative route re-appends its `[Registered, StartRequested, Started]` generation on every boot and the journal grows without bound. The redb open path also retries on lock contention (`DatabaseAlreadyOpen`) to ride out a Kubernetes ReadWriteOnce PVC pod handover instead of crashing.
_Avoid_: log truncation, snapshotting

**Cohort Activation Barrier**:
The context-lifecycle barrier gates the first consumer-envelope dispatch at three drain sites. These are the Concurrent trait branch, the Sequential `_` branch, and the restart aggregate envelope branch. The late aggregator branch is ungated. Its output is post-dispatch by construction. The `reset_cohort` and `activate_cohort` port methods call the shared `Arc` directly. They do not use an actor round-trip. The runtime activates the cohort on every return from boot. A boot failure therefore keeps today's partial-up semantics. Consumers that drive a bare `DefaultRouteController` outside a full context open the same gate through `DefaultRouteController::activate_cohort`; the port methods serve the context path.

**ErrorHandlerConfig**:
Runtime representation of an ErrorHandler — the compiled form of the DSL `ErrorHandler` declaration.
Contains `ExceptionPolicy` list, `DeadLetterChannel` URI, and retry settings applied by the error handler Tower layer.
_Avoid_: error handler (unqualified, use DSL term when describing declaration; use this term when describing runtime type)

**ExceptionPolicy**:
Runtime per-error matching rule compiled from a DSL `OnException`. Uses a predicate over
`CamelError` to select which errors trigger this policy's retry/routing behaviour.
_Avoid_: OnException (use OnException in DSL context; ExceptionPolicy in runtime context)

**In-memory repository max_entries cap (evict-oldest)**:
`MemoryIdempotentRepository` and `MemoryClaimCheckRepository` (in `camel-core`) accept a per-instance `max_entries` cap via `new_with_max_entries(name, cap)`. The default `new()` uses `DEFAULT_MAX_ENTRIES = 100_000`. When the cap is reached for a new key, the OLDEST entry is evicted on the write path (insertion-order via a per-instance atomic seq counter) before the new key is inserted; new-key writes are serialized by a per-instance write guard so the cap invariant holds under concurrent writers. Clock-free, no background task; O(1) amortized under the cap, one O(n) scan per insert-at-cap. Trade-off: an evicted idempotent key re-admits as new (bounded at-most-once ceiling — duplicates possible under a >cap unique-key flood; upgrade path is a persistent repository). Established Batch 1 (H10, H11).

**CacheRepository backends**:
`MemoryCacheRepository` (moka-based, size-eviction only via `eviction_listener` filtered to `RemovalCause::Size`) and `RedbCacheRepository` (persistent, async `new()`, sweep loop bound to `CamelContext::shutdown_token()`). Both implement `CacheRepository` from camel-api (async `stats`). The memory backend is registered as `"memory"` by default with `max_capacity = 10_000`. The redb backend is registered as `"persistent"` when `[default.cache_repo] backend = "redb"` is configured. Redb overrides `invalidate_prefix` with range deletion (memory keeps the fail-closed default). Both backends count `peek_stale_served` and `invalidations` per operation; redb reports `bytes: Some(sum)` (scan inside `spawn_blocking`, bd rc-22wj), degrading to `bytes: None` when the scan fails (unreadable table or a non-deserializable entry) while memory reports `bytes: None`. `RedbCacheRepository::new` requires an explicit `cache_size` (a page-cache byte budget routed through `redb::Builder::set_cache_size`) and warns at open when it exceeds the container's cgroup memory limit. Established by ADR-0056. The redis backend lives outside camel-core, in the `camel-redis-repo` repository service crate (`RedisCacheRepository`, `RedisIdempotentRepository`; ADR-0063): `camel-config` registers it by name when `[default.cache_repo]` or `[default.idempotent_repo]` sets `backend = "redis"`.
**InterceptRules / InterceptAction**:
Ordered interception map for route send points. `struct InterceptRules` and `enum InterceptAction` in `src/intercept.rs` hold exact-URI `InterceptRule` entries (`uri` plus `SkipTo { uri }` or `DivertCopyTo { uri }`). Targets must be `mock:` URIs; `InterceptRules::new` rejects other targets at build time. Lookup is first-match-wins on exact equality. Established by ADR-0064 (see `struct InterceptRules` and `enum InterceptAction`).
_Avoid_: advice, intercept config (use InterceptRules for the set, InterceptAction for the variant)

**Intercept freeze**:
Rules freeze at first successful route registration or at context start, and never unfreeze. `CamelContextBuilder::with_intercept_rules` sets rules at build time before freeze. `CamelContext::set_intercept_rules` installs rules only before freeze and returns `CamelError::Config` after freeze. Freeze lives in `src/lifecycle/adapters/route_controller.rs` (`DefaultRouteController::frozen`, `fn set_intercept_rules`, `fn with_intercept_rules`, `fn mark_started`). Established by ADR-0064.
_Avoid_: late intercept, dynamic intercept

**CompilationContext.intercept**:
The compiler threads the frozen rules into `CompilationContext.intercept` at step resolution. `src/lifecycle/adapters/route_compiler_ext.rs` copies the controller's `InterceptRules` into the context so each `RouteDefinition` compiles against the frozen set; `SkipTo` rewrites the producer URI before endpoint resolution and `DivertCopyTo` composes the copy stage. Established by ADR-0064.
_Avoid_: runtime intercept lookup (use compile-time threading)

**RuntimeComponentMetadataCatalog**:
Thin wrapper around `Arc<Mutex<Registry>>` implementing `ComponentMetadataCatalog`. Created
on-demand via `CamelContext::metadata_catalog()`. Defined in `component_metadata_catalog.rs`.
_Avoid_: metadata catalog impl, catalog wrapper

**Metadata Harvesting**:
The process of calling `component.metadata()` once at registration time (inside
`Registry::register()`) and storing the result indexed by scheme. Ensures metadata is available
without re-invoking the component. The scheme is validated and normalized on mismatch.
_Avoid_: metadata collection, metadata extraction

**Immediate startup grace**:
The observation budget (50ms) of the detached failure watcher spawned with each Immediate
consumer start. On a prompt `start()` error the Route reaches `Failed` within ~grace. No actor
path waits for the grace. Loop-style Immediate consumers (timer, file, sql, cron, keycloak)
surface nothing inside the budget, so the watcher exits when it elapses. `CamelContext::start()`
does not fail fast on Immediate errors. Explicit bind failures still do.

**Outer-task termination watch**:
The detached watcher spawned with each Explicit-class consumer start, resume, and aggregate start after the startup handshake resolves Ok. A task-local drop guard signals termination in every mode (return, panic unwind, abort); the watcher publishes the route failure (crash notification + FailRoute) only when the task ended Pending — no body path accounted the outcome (failure published or normal finally-stop completed) — and the consumer cancel token was not fired. Stop-owned and rollback terminations stay silent. Established by bd rc-a7rh.
_Avoid_: consumer watchdog, join monitor

## Compiled Step Variants

**Process**:
The most common variant — wraps a `BoxProcessor` (a Tower `BoxCloneSyncService<Exchange, Exchange, CamelError>`). Used for all non-structural EIP steps (setBody, log, marshal, etc.).

**Stop**:
Terminates route processing successfully. `run_steps` converts it to `PipelineOutcome::Stopped(ex)` without invoking any Tower service. The exchange state is preserved as-is; the reply channel sees `Ok(ex)` (indistinguishable from Completed).

**Segment**:
Wraps an `OutcomeSegment` — a structural EIP sub-pipeline (Filter, Choice, Loop, Throttle, doTry, Split, StreamingSplit, Multicast, LoadBalance) that returns `PipelineOutcome` directly. Enables `Stopped(ex)` propagation with nested-before-Stop mutations intact.

## Inline dispatch capability publication

At consumer start and resume (route_controller_trait), for non-Concurrent
route shapes EXCEPT top-level aggregate splits (rc-2sba: a split route's
`managed.pipeline` is an identity shell over `compose_pipeline(vec![])` and
must never be inline-executed — aggregate entries stay channel-dispatched),
the controller publishes an `InlineRouteDispatcher`
(`lifecycle/adapters/inline_dispatcher.rs`) onto the fresh `ConsumerContext`:
the live `SharedPipeline` swap source, a child of the pipeline cancellation
token, the shared drain counter, and the cohort gate. Sequential dispatch
through the capability runs admission + snapshot + `pipeline.call` on the
caller's task; consumer stop surfaces `CamelError::ConsumerStopping`. See
CONTEXT-MAP.md "Inline dispatch" (rc-wijd).

## ADR-0012 log-policy annotations

| File | Line | Category | Reason |
|------|------|----------|--------|
| `src/lifecycle/application/commands.rs` | 166 | `system-broken` | Control-plane inconsistency — persist + rollback both failed |

## Example dialogue

> "When a Consumer submits an Exchange, what happens next?"
> "The Runtime wraps it in a UnitOfWorkConfig layer and passes it to the Route's Pipeline. Each Processor transforms or routes it in sequence. When the last Processor completes, the UnitOfWorkConfig fires its completion hooks."
>
> "What is the difference between Exchange and Message?"
> "Exchange is the envelope — it holds two Messages (input and optional output), properties, the exchange pattern, and error state. Message is just body+headers. Components build the initial input Message; Processors read and write it through exchange.input."
>
> "And if a Processor fails?"
> "The failure hooks fire instead. The Exchange carries the error state — the ExchangePattern determines whether a reply with the error is sent back to the caller."
