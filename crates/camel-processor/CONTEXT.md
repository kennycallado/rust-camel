# Processor

EIP (Enterprise Integration Pattern) processors implemented as Tower middleware services. Each
processor compiles from a DSL Step and is composed into the route pipeline.

## EIP catalog

| Module | Processor / concern | Type | Source |
|---|---|---|---|
| `aggregator` | Aggregate | stateful routing / aggregation | `src/aggregator.rs` (`impl AggregatorService`, `fn poll_ready`, `fn call`): defaults are max_buckets=10_000 and bucket_ttl=300s. The background TTL sweep starts lazily on the first `poll_ready` and is managed via `StepLifecycle::start`/`shutdown` (ADR-0022): `shutdown` cancels the sweep token + aborts the handle; `start` resets both so the sweep respawns on route restart. The sweep token is seeded from the constructor's `route_cancel` but lives in an internal swappable cell (`sweep_cancel`), independent of any externally-threaded token. Inline eviction in `call` remains active before the sweep starts (Batch 1 R3-C1). |
| `choice` | Choice | conditional routing | `src/lib.rs:2` |
| `circuit_breaker` | CircuitBreaker gate | fault tolerance | `src/lib.rs:3` |
| `claim_check` | Claim Check | stateful repository stash/retrieve (ADR-0046 retro-exempt) | `src/lib.rs:4` |
| `content_enricher` | Enrich / PollEnrich | enrichment | `src/lib.rs:5` |
| `convert_body` | ConvertBodyTo | transformation | `src/lib.rs:6` |
| `data_format` | built-in data formats | marshal / unmarshal support | `src/lib.rs:7` |
| `delayer` | Delay | timing | `src/lib.rs:8` (header-derived delay clamped to max_delay_ms, default 3_600_000 — Batch 1 H12) |
| `do_try` | doTry / catch | error-handling block | `src/lib.rs:9` |
| `dynamic_router` | DynamicRouter | dynamic routing | `src/lib.rs:11` |
| `dynamic_set_header` | DynamicSetHeader | transformation | `src/lib.rs:12` |
| `dynamic_set_property` | DynamicSetProperty | transformation | `src/lib.rs:13` |
| `endpoint_pipeline` | EndpointPipelineService | endpoint resolution / cache | `src/lib.rs:14` |
| `enrichment_strategy` | EnrichmentStrategy helpers | enrichment | `src/lib.rs:15` |
| `error_handler` | RouteErrorHandler + legacy ErrorHandlerLayer | error handling | `src/lib.rs:16` |
| `filter` | Filter | conditional routing | `src/lib.rs:17` |
| `idempotent_consumer` | Idempotent Consumer | stateful dedup via repository (ADR-0046 retro-exempt) | `src/lib.rs:18` |
| `json_schema_validate` | Validate (JSON Schema) | schema validation | `src/lib.rs:19` |
| `load_balancer` | LoadBalancer | load balancing / failover | `src/lib.rs:20` |
| `log` | Log / DynamicLog | observability | `src/lib.rs:21` |
| `loop_eip` | Loop | repeated routing | `src/lib.rs:22` (Count(n) clamped to config.max_iterations, default MAX_LOOP_ITERATIONS=10_000 — configurable per-step per ADR-0039) |
| `map_body` | MapBody | transformation | `src/lib.rs:23` |
| `marshal` | Marshal / Unmarshal | data format transformation | `src/lib.rs:24` |
| `multicast` | Multicast | fan-out routing | `src/lib.rs:25` |
| `recipient_list` | RecipientList | dynamic recipient routing | `src/lib.rs:27` (expression capped to max_recipients, default 1_000 — Batch 1 H13) |
| `resequencer` | Resequencer (batch/stream) | stateful reorder; continuation boundary (ADR-0029) | `src/lib.rs:28` |
| `routing_slip` | RoutingSlip | dynamic routing | `src/lib.rs:29` |
| `sampling` | Sampling | deterministic 1-of-N (period>0 enforced at build) | `src/lib.rs:30` |
| `script_mutator` | ScriptMutator | script-backed mutation | `src/lib.rs:31` |
| `security_policy_layer` | SecurityPolicyLayer | pre-pipeline authorization | `src/lib.rs:32`; ADR-0010 |
| `set_body` | SetBody | transformation | `src/lib.rs:33` |
| `set_header` | SetHeader | transformation | `src/lib.rs:34` |
| `set_property` | SetProperty | transformation | `src/lib.rs:35` |
| `sort` | Sort | body array sort (no per-call size cap — audit M8) | `src/lib.rs:36` |
| `splitter` | Split | splitting / aggregation | `src/lib.rs:38` |
| `stop` | CompiledStep::Stop | control-flow Stop (ADR-0024) | `CompiledStep` variant (not a `pub mod`); ADR-0025 |
| `stream_cache` | StreamCache | body materialization | `src/lib.rs:39` |
| `stream_codec` | StreamSplitCodec implementations | streaming split support | `src/lib.rs:40` |
| `streaming_splitter` | Streaming Split | streaming splitter | `src/lib.rs:42` |
| `throttler` | Throttle | rate limiting | `src/lib.rs:43` (try_new returns Err on max_requests=0 or period=0 — Batch 1 D-M8) |
| `validate` | Validate (predicate) | predicate validation wrapper | `src/lib.rs:44` |
| `wire_tap` | WireTap | fire-and-forget side route with bounded-admission semaphore (max_concurrent cap, CallerRuns at bound) | `src/lib.rs:45` |
| `zip_splitter` | ZipSplitter | archive splitting | `src/lib.rs:46` |

## Structural EIP Segments

Phase 4 (ADR-0025) migrated structural EIPs from Tower services returning `Err(CamelError::Stopped)` to `OutcomePipeline` implementations that return `PipelineOutcome` directly. Each wraps its body as an `OutcomeSegment` and is stored in a `CompiledStep::Segment` variant.

| Segment | Module | Description |
|---------|--------|-------------|
| `FilterSegment` | `filter` | Conditional routing — if predicate matches, runs body; else skips. Stop from body propagates to outer pipeline. |
| `ChoiceSegment` | `choice` | Conditional routing — evaluates `when` clauses in order; first match runs its body. Stop from body propagates. |
| `LoopSegment` | `loop_eip` | Repeated routing — runs body configured number of times (or until Stop). Stop from body breaks immediately. |
| `ThrottleSegment` | `throttler` | Rate limiting — delays body execution to enforce max throughput. Stop from body propagates. |
| `DoTrySegment` | `do_try` | Error-handling block — runs body; on error, matches `catch` clauses. Stop from body or catch propagates. |
| `SplitSegment` | `split_segment` | Splitting / aggregation — splits Exchange by expression, runs body per fragment, aggregates results. Stop from body returns `Stopped(fragment_ex)`, skips aggregation. `SplitterService` (Tower wrapper) delegates to this. |
| `StreamingSplitSegment` | `streaming_split_segment` | Streaming split — same semantics as `SplitSegment` but processes a lazy byte stream. Stop drops the underlying stream and returns `Stopped(fragment_ex)`. |
| `MulticastSegment` | `multicast` | Fan-out routing — sends Exchange to multiple endpoints in parallel or sequentially. Stop from any branch propagates to outer pipeline. |
| `LoadBalanceSegment` | `load_balancer` | Load balancing / failover — selects one endpoint per strategy (round-robin, random, failover). Stop from selected branch propagates. |
| `IdempotentConsumerSegment` | `idempotent_consumer` | Dedup — backed by `Arc<dyn IdempotentRepository>` (`camel-api/src/idempotent.rs:21`); computes message id via `MessageIdExpression`, checks `contains(key)`, runs body + records key on first sighting, skips body on duplicate. Stop from body propagates. |

## Public API surface (v1.0 freeze)

Status values: `stable` means normal public API, `deprecated` means Rust deprecation attribute is present, and `legacy-pending-removal` means retained only for a documented migration window.

| Export | Status | Source | Notes |
|---|---|---|---|
| `AggregatorService` | stable | `pub use aggregator::AggregatorService` | Aggregate EIP service. |
| `ChoiceSegment`, `ChoiceService`, `WhenClause`, `WhenClauseSegment` | stable | `pub use choice::{...}` | Choice EIP. |
| `CircuitBreakerDecision`, `CircuitBreakerGate`, `CircuitBreakerLayer`, `CircuitBreakerService` | stable | `pub use circuit_breaker::{...}` | README claims deprecation for layer/service; code has no `#[deprecated]` yet. |
| `ClaimCheckOp`, `ClaimCheckService`, `KeyExpression` | stable | `pub use claim_check::{...}` | Claim Check EIP. |
| `EnrichService`, `PollEnrichService` | stable | `pub use content_enricher::{...}` | Enrich / pollEnrich. |
| `ConvertBodyTo` | stable | `pub use convert_body::ConvertBodyTo` | Body conversion. |
| `CsvConfig`, `CsvDataFormat`, `QuoteMode`, `RecordSeparator`, `CAMEL_CSV_HEADER_RECORD`, `JsonConfig`, `JsonDataFormat`, `XmlConfig`, `XmlDataFormat`, `ZipConfig`, `ZipDataFormat`, `builtin_data_format`, `builtin_data_format_with_config` | stable | `pub use data_format::{...}` | Built-in data formats. CSV added per ADR-0030. Configurable DoS caps per ADR-0038. |
| `DelayerService` | stable | `pub use delayer::DelayerService` | Delay EIP. |
| `CatchClause`, `CatchMatcher`, `DoTryService` | stable | `pub use do_try::{...}` | doTry Tower service. |
| `CatchClauseSegment`, `DoTrySegment`, `FinallyClauseSegment` | stable | `pub use do_try_segment::{...}` | Outcome-aware doTry segment. |
| `DynamicRouterService` | stable | `pub use dynamic_router::DynamicRouterService` | Dynamic router. |
| `DynamicSetHeader`, `DynamicSetHeaderIfAbsent`, `DynamicSetHeaderLayer` | stable | `pub use dynamic_set_header::{...}` | Header mutation. |
| `DynamicSetProperty`, `DynamicSetPropertyLayer` | stable | `pub use dynamic_set_property::{...}` | Property mutation. |
| `EndpointPipelineService` | stable | `pub use endpoint_pipeline::EndpointPipelineService` | Endpoint resolver/cache wrapper. |
| `EnrichmentStrategy`, `ThrowOnNoPoll`, `UseEnrichedBody` | stable | `pub use enrichment_strategy::{...}` | Enrichment merge strategy. |
| `DefaultRouteErrorHandler`, `RouteErrorHandler`, `invoke_processor` | stable | `pub use error_handler::{...}` | ADR-0019 in-pipeline handler API. |
| `ErrorHandlerLayer`, `ErrorHandlerService` | stable | `pub use error_handler::{...}`; `struct ErrorHandlerLayer`, `struct ErrorHandlerService` | Legacy Tower layer/service for in-pipeline error handling; prefer `RouteChannelService` + `DefaultRouteErrorHandler` per ADR-0019 for new code. |
| `FilterSegment`, `FilterService` | stable | `pub use filter::{...}` | Filter EIP. |
| `IdempotentConsumerSegment`, `MessageIdExpression` | stable | `pub use idempotent_consumer::{...}` | Repository-backed idempotent consumer. |
| `JsonSchemaValidateService` | stable | `pub use json_schema_validate::JsonSchemaValidateService` | JSON Schema validation. |
| `LoadBalanceSegment`, `LoadBalancerService` | stable | `pub use load_balancer::{...}` | Load balancer EIP; service poll_ready migration pending. |
| `LogLevel`, `LogProcessor` | stable | `pub use log::{...}` | Log EIP. |
| `CAMEL_LOOP_INDEX`, `CAMEL_LOOP_SIZE`, `LoopSegment`, `LoopService` | stable | `pub use loop_eip::{...}` | Loop EIP metadata, segment, and service. |
| `MapBody`, `MapBodyLayer` | stable | `pub use map_body::{...}` | Body mapping. |
| `MarshalService`, `UnmarshalService` | stable | `pub use marshal::{...}` | Data format transform. |
| `CAMEL_MULTICAST_COMPLETE`, `CAMEL_MULTICAST_INDEX`, `MulticastService` | stable | `pub use multicast::{...}` | Multicast metadata + service. |
| `MulticastSegment` | stable | `pub use multicast_segment::MulticastSegment` | Outcome-aware multicast segment. |
| `RecipientListService` | stable | `pub use recipient_list::RecipientListService` | Recipient list EIP. |
| `RoutingSlipService` | stable | `pub use routing_slip::RoutingSlipService` | Routing slip EIP. |
| `SamplingService` | stable | `pub use sampling::SamplingService` | Sampling EIP. |
| `ScriptMutator` | stable | `pub use script_mutator::ScriptMutator` | Script mutation. |
| `SecurityPolicyLayer`, `SecurityPolicyService` | stable | `pub use security_policy_layer::{...}` | Pre-pipeline authorization per ADR-0010. |
| `SetBody`, `SetBodyLayer` | stable | `pub use set_body::{...}` | Body mutation. |
| `SetHeader`, `SetHeaderIfAbsent`, `SetHeaderLayer` | stable | `pub use set_header::{...}` | Header mutation. |
| `SetProperty`, `SetPropertyLayer` | stable | `pub use set_property::{...}` | Property mutation. |
| `SortExpression`, `SortKey`, `SortService` | stable | `pub use sort::{...}` | Body sort EIP. |
| `SplitSegment`, `SplitterService` | stable | `pub use split_segment::SplitSegment`; `pub use splitter::SplitterService` | Split EIP; service poll_ready migration pending. |
| `StreamCacheService` | stable | `pub use stream_cache::StreamCacheService` | Stream body materialization. |
| `StreamingSplitSegment`, `StreamingSplitterService` | stable | `pub use streaming_split_segment::StreamingSplitSegment`; `pub use streaming_splitter::StreamingSplitterService` | Streaming Split EIP; service poll_ready migration pending. |
| `ThrottleSegment`, `ThrottlerService` | stable | `pub use throttler::{...}` | Throttle EIP. |
| `ValidateService` | stable | `pub use validate::ValidateService` | Predicate validation. |
| `WireTapConfig`, `WireTapLayer`, `WireTapLifecycle`, `WireTapService` | stable | `pub use wire_tap::{...}` | WireTap EIP; poll_ready migrated per ADR-0019. `WireTapLifecycle` is the lifecycle handle for graceful-drain-then-abort teardown (ADR-0022). |
| `BatchPolicy`, `StreamPolicy`, `PassthroughPolicy`, `ResequencePolicy`, `ResequencerConfig`, `ResequencerService` | stable | `pub use resequencer...` | Resequencer policies, config, and service. |

## poll_ready contract

ADR-0019 requires processors whose errors are routable/recoverable events to avoid treating Tower readiness errors as permanent service breakage. Those processors MUST return `Ready(Ok(()))` from `poll_ready` and defer endpoint readiness to `call()`. ADR-0010 is the explicit exception for `SecurityPolicyService`: pre-pipeline authorization faults must surface before data-plane processing.

| Processor | poll_ready behavior | Status | Rationale / source |
|---|---|---|---|
| `MulticastService` | `Ready(Ok(()))` unconditional | migrated | Per-endpoint readiness happens inside fan-out logic. See `fn poll_ready` and `fn call` in `impl Service<Exchange> for MulticastService`. |
| `ErrorHandlerService` | preserves `Pending`, maps readiness `Err` to `Ok(())` | migrated | Deprecated compatibility shell still follows ADR-0019. See `fn poll_ready` in `impl Service<Exchange> for ErrorHandlerService`. |
| `AggregatorService` | starts the TTL sweep lazily, then returns `Ready(Ok(()))` | migrated | Readiness does not probe an endpoint. `call` owns aggregation state and also performs inline TTL eviction. See `fn poll_ready` and `fn call` in `impl Service<Exchange> for AggregatorService`. |
| `RecipientListService` | `Ready(Ok(()))` unconditional | migrated | Dynamic recipients resolve and check readiness in `call()`. See `fn poll_ready` and `fn call` in `impl Service<Exchange> for RecipientListService`. |
| `WireTapService` | `Ready(Ok(()))` unconditional | migrated | Fire-and-forget tap readiness is checked inside the tap task; the main route never blocks on the tap (ADR-0019). See `fn poll_ready` in `impl Service<Exchange> for WireTapService`. |
| `LoadBalancerService` | polls all endpoints and returns first `Err` | pending-fix | Failover/selection must happen in `call()`. See `fn poll_ready` and `fn call` in `impl Service<Exchange> for LoadBalancerService`. |
| `SplitterService` | delegates to sub-pipeline | pending-fix | Fragment processing checks readiness in `call()` through `process_sequential` or `process_parallel`. See `fn poll_ready` in `impl Service<Exchange> for SplitterService`. |
| `StreamingSplitterService` | delegates to sub-pipeline | pending-fix | Streaming fragment processing checks readiness in `call()`. See `fn poll_ready` and `fn call` in `impl Service<Exchange> for StreamingSplitterService`. |
| `FilterService` | delegates to sub-pipeline | pending-fix | Predicate evaluation and conditional dispatch happen in `call()`. See `fn poll_ready` and `fn call` in `impl Service<Exchange> for FilterService`. |
| `SecurityPolicyService` | delegates to inner service | excluded | Authz faults are outside normal EIP recovery per ADR-0010. See `fn poll_ready` in `impl Service<Exchange> for SecurityPolicyService`. |

## Language

**StreamSplitCodec**:
Trait in `stream_codec.rs` that splits a byte stream into fragment Exchanges. Takes a `StreamSplitInput` (parent Exchange + byte stream + metadata) and produces a `Stream<Item = Result<Exchange>>`. Built-in implementations:
- `NdjsonCodec` — splits on newline-delimited JSON boundaries, parses each line as JSON into the fragment body.
- `LinesCodec` — splits on newline boundaries, each line becomes the fragment body as a UTF-8 string.
- `ChunksCodec` — splits into fixed-size byte chunks (requires `chunk_size` in config).
- `Auto` — resolves format from `StreamMetadata.content_type` (`application/x-ndjson` → Ndjson, `text/plain` → Lines, `application/octet-stream` → Chunks).

**CamelStreamOrigin**:
Exchange property (`"CamelStreamOrigin"`) set on each fragment to trace it back to the parent stream. Value is a UUID generated once per `StreamSplitInput`.

**CamelStreamSourceContentType**:
Exchange property (`"CamelStreamSourceContentType"`) set on each fragment carrying the original stream's content type.

**CamelStreamOffset**:
Exchange property (`"CamelStreamOffset"`) set on each fragment — monotonically increasing zero-based index of the fragment within the stream.

**CamelStreamBatchSize**:
Exchange property (`"CamelStreamBatchSize"`) set on each fragment — total number of fragments expected in this batch (not set for streaming splits where count is unknown).

**CamelCsvHeaderRecord**:
Exchange header key (`"CamelCsvHeaderRecord"`) populated by `CsvDataFormat::unmarshal_in_exchange` when `CsvConfig.capture_header_record=true`. Value: `Value::Array` of `Value::String` containing the CSV header keys. Independent of `use_maps` mode.

**StreamSplitInput**:
Groups the parent Exchange, the byte stream (`Pin<Box<dyn Stream<Item = Result<Bytes, CamelError>>>>`), and `StreamMetadata` into one argument for `StreamSplitCodec::split`.

**Header exclusion**:
`fragment_stream_exchange` strips `Content-Length` and `Content-Type` from fragment headers. These headers belong to the parent stream body, not individual fragments. See `fn fragment_stream_exchange` in `src/stream_codec.rs`.

**fragment_stream_exchange**:
Wraps `fragment_exchange` (camel-api) with stream-specific header exclusion. Used by all three built-in codecs to create each fragment Exchange from the parent Exchange and a parsed body.

YAML example (streaming NDJSON split):

```yaml
- split:
    streaming: true
    stream:
      format: ndjson
    steps:
      - to: "log:fragment"
```

## Aggregation contract (divergence from Apache Camel)

`SplitterService` and `StreamingSplitterService` invoke the configured
`AggregationStrategy` with a `Vec<Result<Exchange, CamelError>>` — failed
fragments appear as `Err(e)` entries in the Vec, NOT as Exchanges with the
exception attached. This diverges permanently from Apache Camel, where the
aggregation strategy receives the failed Exchange and inspects
`exchange.getProperty(Exchange.EXCEPTION_CLASS)` to decide recovery.

The rust-camel model is forced by ADR-0019 (`ExceptionDisposition` enum
replaces `handled:bool`) and the Tower separation (`Service<Exchange>`
returns `Result<Exchange, CamelError>`, not Exchange-with-exception-state).
Aggregation strategies that need error-aware logic must inspect the `Result`
shape; they cannot rely on the Camel-style exception-attached Exchange.

Per ADR-0046 §Anti-patrones, this divergence is documented here as tracked
content (not in a gitignored spike doc). Source: spike Splitter (commit
`8d31e74a`), divergence label D2.

## Stateful repository EIPs (ADR-0046 retro-exempt)

`IdempotentConsumerSegment` (`struct IdempotentConsumerSegment` and `impl OutcomePipeline for IdempotentConsumerSegment` in `src/idempotent_consumer.rs`) and `ClaimCheckService` (`struct ClaimCheckService` in `src/claim_check.rs`) are stateful EIPs backed by a repository trait, implemented 2026-06-27/28 (before ADR-0046 was accepted 2026-07-17). They are **retroactive-exempt** from the ADR-0046 consultation protocol per §Scope: no Camel test-mining is required. This section documents their contract **from existing behaviour** (not from a Camel comparison) so a future contributor understands the shape without re-deriving it.

- **Idempotent Consumer** — backed by `Arc<dyn IdempotentRepository>` (`trait IdempotentRepository` in `camel-api/src/idempotent.rs`). The segment computes a message id via `MessageIdExpression` (`type MessageIdExpression` in `src/idempotent_consumer.rs`), checks `contains(key)`, and either runs the body + records the key (first sighting) or skips the body (duplicate). Maps to Camel's `IdempotentConsumer` / `MessageIdRepository`; the repository abstraction is the rust-camel analogue.
- **Claim Check** — backed by `Arc<dyn ClaimCheckRepository>` (`trait ClaimCheckRepository` in `camel-api/src/claim_check.rs`), driven by `ClaimCheckOp` (Set / Get / GetAndRemove / Push / Pop) and `KeyExpression` (`type KeyExpression` in `src/claim_check.rs`). Stash/retrieve of the message body against a key. Maps to Camel's Claim Check EIP operation set.

If a concrete design question about divergence from Camel emerges for either EIP, it is escalated case-by-case at that point (ADR-0046 §Scope); until then the documented contract above is sufficient. No `gap-coverage` bd or mandatory protocol is opened — these are voluntary-resolution entries (blind spot #16), not `FC-BEHAVIORAL-PARITY-GAP`.

## ADR-0012 log-policy sites

The crate has 11 annotated log-policy sites. Seven **(a) handler-owned** sites use `warn!` because
the route ErrorHandler owns ERROR responsibility. Three **system-broken** sites use `error!`. One
post-ack best-effort site uses `warn!` and reports a metric under ADR-0029.

| File | Symbol / site | Category | Annotation |
|------|---------------|----------|------------|
| `aggregator.rs` | `AggregatorService::force_complete_all` | (a) handler-owned | `// log-policy: handler-owned` — force-complete aggregation failed |
| `aggregator.rs` | `fn spawn_timeout_task` | (a) handler-owned | `// log-policy: handler-owned` — timeout aggregation failed |
| `log.rs` | `LogProcessor::call`, `LogLevel::Error` arm | (a) handler-owned | `// log-policy: handler-owned` — default Error level |
| `log.rs` | `DynamicLog::call`, `LogLevel::Error` arm | (a) handler-owned | `// log-policy: handler-owned` — default Error level |
| `wire_tap.rs` | `WireTapService::call` | (a) handler-owned | `// log-policy: handler-owned` — processing error |
| `resequencer/batch.rs` | `BatchPolicy::accept` | (a) handler-owned | `// log-policy: handler-owned` — correlation expression failed |
| `error_handler.rs` | `fn execute_on_steps` | (a) handler-owned | `// log-policy: handler-owned` — on-steps pipeline failed |
| `resequencer/mod.rs` | `ResequencerService::with_config` post-driver task | post-ack best-effort | `// log-policy: post-ack failure (ADR-0012 best-effort, ADR-0029 I7)` — continuation call failed |
| `error_handler.rs` | `fn send_to_handler`, no producer | system-broken | `// log-policy: system-broken` — no error handler configured |
| `error_handler.rs` | `fn send_to_handler`, producer not ready | system-broken | `// log-policy: system-broken` — DLC/handler not ready |
| `error_handler.rs` | `fn send_to_handler`, producer call failed | system-broken | `// log-policy: system-broken` — DLC/handler call failed |

## Metrics

Metrics instrumentation is not yet wired for most processors. See TODO(PROC-004) in
`camel-processor/src/log.rs` for the broader instrumentation gap.

## Aggregator EIP divergences from Apache Camel (ADR-0046 protocol)

This section records divergences surfaced by applying the ADR-0046 protocol to the Aggregator EIP;
each divergence names the forcing contract shape or ADR and the observable consequence. The
Splitter-specific divergence D2 lives in the separate "Aggregation contract (divergence from Apache
Camel)" block above.

### D-A1: binary-fold strategy contract — no null oldExchange on first message

Apache Camel's `AggregationStrategy.aggregate(Exchange oldExchange, Exchange newExchange)` receives
`null` for `oldExchange` on the FIRST message of a bucket, letting a strategy initialize.
rust-camel's `AggregationFn = Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>` (defined in
`crates/camel-api/src/aggregator.rs`, the `AggregationFn` alias) ALWAYS receives two exchanges: the first message sits
untouched in the bucket, and the strategy is first invoked as `f(ex1, ex2)` when the second
message arrives. Forcing contract shape: the binary `AggregationFn` signature + the bucket model
that retains the first exchange as the fold seed. Observable consequence: a strategy needing
initialize-on-first logic must branch on a sentinel in the accumulated body (or check a property)
rather than on a null oldExchange. Pinned by `aggregator::tests::test_da1_strategy_receives_two_exchanges_first_message_preserved`.

### D-A2: AggregationFn cannot signal failure — no Result return

Apache Camel's `AggregationStrategy.aggregate()` may throw, propagating the exception and failing the
aggregated exchange. rust-camel's `AggregationFn` returns `Exchange` (not
`Result<Exchange, CamelError>`), so a custom strategy has no path to signal invalid aggregation
through the return type except by panicking. Forcing contract shape: the `AggregationFn` alias at
`crates/camel-api/src/aggregator.rs` — `Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>`.
This is the D2-family divergence for the Aggregate EIP specifically — distinct from the Splitter
EIP's `Vec<Result<Exchange, CamelError>>` shape documented in the "Aggregation contract" block
above. Observable consequence: error-aware aggregation logic cannot be expressed in the strategy
return; it must live outside (e.g. in a downstream doTry/error-handler per ADR-0019) or in a
wrapping service.

### D-A3: force-completion-on-stop channel-mediated emission + drop under pressure

Apache Camel's `forceCompletionOnStop()` flows pending buckets synchronously through the downstream
pipeline during `context.stop()`. rust-camel's `force_complete_all()` (in
`crates/camel-processor/src/aggregator.rs`, the `force_complete_all` method) is nonblocking
(`-> ()`): it cannot return completed exchanges inline, so it emits them through a bounded `late_tx`
mpsc channel (capacity 256, see `crates/camel-core/src/lifecycle/adapters/route_controller.rs` where
the channel is created) that a `select!` arm drains into the post-pipeline. Boolean semantics are
equal: `force_completion_on_stop == true` emits pending buckets, `== false` drops them. Two
divergences: (1) channel-mediated async emission vs Camel's synchronous flow; (2) under
late-channel-full pressure, `try_send` fails and the force-completed exchange is DROPPED with a
`warn!` log (the "aggregator force-complete emit dropped" warn site in `force_complete_all`) — Camel
has no equivalent drop path. Forcing contract shape: nonblocking `force_complete_all() -> ()`
(the nonblocking signature is forced by the Tower `Service` shutdown lifecycle — shutdown cannot
block on inline return) plus the bounded `late_tx` (capacity 256) that the route controller's
`select!` arm drains. Pinned by
`aggregator::tests::test_da3_force_complete_all_drops_on_saturated_channel`.

### D-A4: per-bucket timeout task vs central completion-timeout-checker + knob divergence

Apache Camel runs a single background `completionTimeoutChecker` thread that polls all
buckets every `completionTimeoutCheckerInterval(ms)` to find expired ones. rust-camel
uses a per-bucket dedicated tokio task spawned by `spawn_timeout_task` (in
`crates/camel-processor/src/aggregator.rs`, the `spawn_timeout_task` function), cancelled
and reset on each new exchange for that key, PLUS a `bucket_ttl` background sweep
(interval `ttl/2`, floor 50ms) as a fallback eviction path, PLUS a `max_timeout_tasks`
DoS cap that gracefully degrades to TTL-only eviction when the cap is reached. The
observable completion semantics are EQUAL (a bucket completes after the configured
inactivity period) but the MECHANISM differs.

Knob divergence: rust-camel exposes `max_timeout_tasks` and `bucket_ttl` (Camel does not);
Camel exposes `completionTimeoutCheckerInterval` (rust-camel does not — the per-bucket task
makes it unnecessary). Forcing contract shape: `CompletionCondition::Timeout` (in
`crates/camel-api/src/aggregator.rs`), `AggregatorConfig.max_timeout_tasks`, and
`AggregatorConfig.bucket_ttl` (same file) — all three configuration contracts already defined
in `AggregatorConfig`. Pinned by
`aggregator::tests::test_timeout_completes_bucket`,
`aggregator::tests::test_timeout_resets_on_new_exchange`,
`aggregator::tests::test_bucket_ttl_eviction`, and
`aggregator::tests::test_aggregator_timeout_task_cap_no_panic_under_flood`.

### D-A5: mandatory memory bounds — validate() rejects unbounded configs

Apache Camel's default in-memory aggregation repository is UNBOUNDED (no mandatory cap; the operator
may configure one). rust-camel's `AggregatorConfig::validate()` (the `validate` method in
`crates/camel-api/src/aggregator.rs`) REJECTS any config with no memory-release bound — it returns
`CamelError::ConfigValidation(ConfigValidationError::AggregatorMissingMemoryBound)` when none of
`max_buckets`, a `Timeout` completion condition, or `bucket_ttl` is set, and
`ConfigValidationError::AggregatorTimeoutRequiresTtl` when a `Timeout` completion is present without
`bucket_ttl`. The builder defaults are `max_buckets = 10_000` and `bucket_ttl = 300s`. Forcing ADR:
ADR-0033 (security defaults — typed `ConfigValidationError`, operators may match on the variant).
Consequence for an operator migrating from Camel: a config that is valid (if risky) in Camel may be
REJECTED at build/validate time here; the operator must set an explicit bound. Pinned by
`camel_api::aggregator::tests::test_aggregator_config_rejects_no_memory_bound` (substring check) and
`camel_api::aggregator::tests::test_da5_validate_returns_typed_missing_memory_bound_variant` (typed-variant pin).

### G-A1 (gap-coverage): completionSize as Expression — static Size only

Apache Camel supports `completionSize(expression)` where the size limit is evaluated per-exchange
(e.g. derived from a header on each incoming message). rust-camel's
`CompletionCondition::Size(usize)` (in `crates/camel-api/src/aggregator.rs`) is STATIC — the limit
is a fixed `usize` set at config time with no per-exchange evaluation path. This is a COVERAGE GAP
(less surface), NOT a forced divergence: no ADR forbids an expression-based size; it is simply not
yet implemented. Implementing it is out of scope for this change (which is documentation-only per
ADR-0046). A future feature task may add a `CompletionCondition::SizeExpr { expr, language }`
variant mirroring the existing `PredicateExpr` variant (same enum, same file).

## WireTap EIP divergences from Apache Camel (ADR-0046 protocol)

This section records divergences surfaced by applying the ADR-0046 protocol to the WireTap EIP.
Each divergence names the forcing contract shape or ADR and the observable consequence.

### D-W1: flat-semaphore admission collapse

Apache Camel uses a two-tier admission model: `maxPoolSize=20` (thread pool cap) plus
`maxQueueSize=1000` (backlog queue depth). rust-camel collapses both tiers into a single flat
concurrency cap backed by a `tokio::sync::Semaphore` with `CallerRuns` at the bound.
Forcing rationale: Camel's own virtual-thread executor is documented as exactly this
semaphore-based flat cap. Observable consequence: operators configure one bound
(`max_concurrent`), not pool size plus queue depth.

### D-W2: CallerRuns transient exceed

Under saturation the inline task makes total concurrent execution reach `bound + 1` transiently.
Forcing rationale: `CallerRuns` runs the tap on the caller's thread rather than queueing it.
Observable consequence: peak concurrency is `bound + number_of_saturated_callers`, not exactly
`bound`.

### D-W3: route-level teardown, not CamelContext-level

WireTap taps are drained or aborted at route `shutdown` (ADR-0022 `StepLifecycle`), not at
CamelContext shutdown. Forcing rationale: rust-camel has no global `CamelContext` shutdown hook;
the route lifecycle owns the drain. Observable consequence: stopping a route drains its taps;
there is no cross-route pool.

### D-W4: absent pool-profile knobs

Apache Camel exposes `poolSize`, `maxPoolSize`, `maxQueueSize`, `rejectedPolicy`, and executor
service references. rust-camel exposes only `max_concurrent` and `shutdown_grace`.
Forcing rationale: the flat-semaphore model (D-W1) makes the pool and queue knobs redundant.
Observable consequence: operators cannot tune the queue depth or choose a rejection policy
other than `CallerRuns`.
