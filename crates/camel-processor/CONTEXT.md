# Processor

EIP (Enterprise Integration Pattern) processors implemented as Tower middleware services. Each
processor compiles from a DSL Step and is composed into the route pipeline.

## EIP catalog

| Module | Processor / concern | Type | Source |
|---|---|---|---|
| `aggregator` | Aggregate | stateful routing / aggregation | `src/lib.rs:1` |
| `choice` | Choice | conditional routing | `src/lib.rs:2` |
| `circuit_breaker` | CircuitBreaker gate | fault tolerance | `src/lib.rs:3` |
| `content_enricher` | Enrich / PollEnrich | enrichment | `src/lib.rs:4` |
| `convert_body` | ConvertBodyTo | transformation | `src/lib.rs:5` |
| `data_format` | built-in data formats | marshal / unmarshal support | `src/lib.rs:6` |
| `delayer` | Delay | timing | `src/lib.rs:7` |
| `do_try` | doTry / catch | error-handling block | `src/lib.rs:8` |
| `dynamic_router` | DynamicRouter | dynamic routing | `src/lib.rs:9` |
| `dynamic_set_header` | DynamicSetHeader | transformation | `src/lib.rs:10` |
| `dynamic_set_property` | DynamicSetProperty | transformation | `src/lib.rs:11` |
| `endpoint_pipeline` | EndpointPipelineService | endpoint resolution / cache | `src/lib.rs:12` |
| `enrichment_strategy` | EnrichmentStrategy helpers | enrichment | `src/lib.rs:13` |
| `error_handler` | RouteErrorHandler + legacy ErrorHandlerLayer | error handling | `src/lib.rs:14` |
| `filter` | Filter | conditional routing | `src/lib.rs:15` |
| `load_balancer` | LoadBalancer | load balancing / failover | `src/lib.rs:16` |
| `log` | Log / DynamicLog | observability | `src/lib.rs:17` |
| `loop_eip` | Loop | repeated routing | `src/lib.rs:18` |
| `map_body` | MapBody | transformation | `src/lib.rs:19` |
| `marshal` | Marshal / Unmarshal | data format transformation | `src/lib.rs:20` |
| `multicast` | Multicast | fan-out routing | `src/lib.rs:21` |
| `recipient_list` | RecipientList | dynamic recipient routing | `src/lib.rs:22` |
| `routing_slip` | RoutingSlip | dynamic routing | `src/lib.rs:23` |
| `script_mutator` | ScriptMutator | script-backed mutation | `src/lib.rs:24` |
| `security_policy_layer` | SecurityPolicyLayer | pre-pipeline authorization | `src/lib.rs:25`; ADR-0010 |
| `set_body` | SetBody | transformation | `src/lib.rs:26` |
| `set_header` | SetHeader | transformation | `src/lib.rs:27` |
| `set_property` | SetProperty | transformation | `src/lib.rs:28` |
| `splitter` | Split | splitting / aggregation | `src/lib.rs:29` |
| `stop` | CompiledStep::Stop | control-flow Stop (ADR-0024) | `src/lib.rs:30`; ADR-0025 |
| `stream_cache` | StreamCache | body materialization | `src/lib.rs:31` |
| `stream_codec` | StreamSplitCodec implementations | streaming split support | `src/lib.rs:32` |
| `streaming_splitter` | StreamingSplit | streaming splitter | `src/lib.rs:33` |
| `throttler` | Throttle | rate limiting | `src/lib.rs:34` |
| `wire_tap` | WireTap | fire-and-forget side route | `src/lib.rs:35` |
| `zip_splitter` | ZipSplitter | archive splitting | `src/lib.rs:36` |

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

## Public API surface (v1.0 freeze)

Status values: `stable` means normal public API, `deprecated` means Rust deprecation attribute is present, and `legacy-pending-removal` means retained only for a documented migration window.

| Export | Status | Source | Notes |
|---|---|---|---|
| `AggregatorService` | stable | `src/lib.rs:38` | Aggregate EIP service. |
| `ChoiceService`, `WhenClause` | stable | `src/lib.rs:39` | Choice EIP. |
| `CircuitBreakerDecision`, `CircuitBreakerGate`, `CircuitBreakerLayer`, `CircuitBreakerService` | stable | `src/lib.rs:40-42` | README claims deprecation for layer/service; code has no `#[deprecated]` yet. |
| `EnrichService`, `PollEnrichService` | stable | `src/lib.rs:43` | Enrich / pollEnrich. |
| `ConvertBodyTo` | stable | `src/lib.rs:44` | Body conversion. |
| `CsvConfig`, `CsvDataFormat`, `QuoteMode`, `RecordSeparator`, `CAMEL_CSV_HEADER_RECORD`, `JsonDataFormat`, `XmlDataFormat`, `ZipDataFormat`, `builtin_data_format` | stable | `src/lib.rs:55` | Built-in data formats. CSV added per ADR-0030. |
| `DelayerService` | stable | `src/lib.rs:46` | Delay EIP. |
| `CatchClause`, `CatchMatcher`, `DoTryService` | stable | `src/lib.rs:47` | doTry block. |
| `DynamicRouterService` | stable | `src/lib.rs:48` | Dynamic router. |
| `DynamicSetHeader`, `DynamicSetHeaderLayer` | stable | `src/lib.rs:49` | Header mutation. |
| `DynamicSetProperty`, `DynamicSetPropertyLayer` | stable | `src/lib.rs:50` | Property mutation. |
| `EndpointPipelineService` | stable | `src/lib.rs:51` | Endpoint resolver/cache wrapper. |
| `EnrichmentStrategy`, `UseEnrichedBody` | stable | `src/lib.rs:52` | Enrichment merge strategy. |
| `DefaultRouteErrorHandler`, `RouteErrorHandler`, `invoke_processor` | stable | `src/lib.rs:55-58` | ADR-0019 in-pipeline handler API. |
| `ErrorHandlerLayer`, `ErrorHandlerService` | stable | `src/lib.rs:55-58`; `src/error_handler.rs:367-420` | Legacy Tower layer/service for in-pipeline error handling; prefer `RouteChannelService` + `DefaultRouteErrorHandler` per ADR-0019 for new code. |
| `FilterService` | stable | `src/lib.rs:59` | Filter EIP. |
| `LoadBalancerService` | stable | `src/lib.rs:60` | Load balancer EIP; poll_ready migration pending. |
| `LogLevel`, `LogProcessor` | stable | `src/lib.rs:61` | Log EIP. |
| `CAMEL_LOOP_INDEX`, `CAMEL_LOOP_SIZE`, `LoopService` | stable | `src/lib.rs:62` | Loop EIP metadata + service. |
| `MapBody`, `MapBodyLayer` | stable | `src/lib.rs:63` | Body mapping. |
| `MarshalService`, `UnmarshalService` | stable | `src/lib.rs:64` | Data format transform. |
| `CAMEL_MULTICAST_COMPLETE`, `CAMEL_MULTICAST_INDEX`, `MulticastService` | stable | `src/lib.rs:65` | Multicast metadata + service. |
| `RecipientListService` | stable | `src/lib.rs:66` | Recipient list EIP. |
| `RoutingSlipService` | stable | `src/lib.rs:67` | Routing slip EIP. |
| `ScriptMutator` | stable | `src/lib.rs:68` | Script mutation. |
| `SecurityPolicyLayer`, `SecurityPolicyService` | stable | `src/lib.rs:69` | Pre-pipeline authorization per ADR-0010. |
| `SetBody`, `SetBodyLayer` | stable | `src/lib.rs:70` | Body mutation. |
| `SetHeader`, `SetHeaderLayer` | stable | `src/lib.rs:71` | Header mutation. |
| `SetProperty`, `SetPropertyLayer` | stable | `src/lib.rs:72` | Property mutation. |
| `SplitterService` | stable | `src/lib.rs:73` | Split EIP; poll_ready migration pending. |
| `CompiledStep::Stop` | stable | `src/lib.rs:74` | Control-flow Stop EIP compiled step (ADR-0024, ADR-0025). |
| `StreamCacheService` | stable | `src/lib.rs:75` | Stream body materialization. |
| `StreamingSplitterService` | stable | `src/lib.rs:76` | Streaming Split EIP; poll_ready migration pending. |
| `ThrottlerService` | stable | `src/lib.rs:77` | Throttle EIP. |
| `WireTapConfig`, `WireTapLayer`, `WireTapService` | stable | `src/lib.rs:78` | WireTap EIP; poll_ready migration pending. |

## poll_ready contract

ADR-0019 requires processors whose errors are routable/recoverable events to avoid treating Tower readiness errors as permanent service breakage. Those processors MUST return `Ready(Ok(()))` from `poll_ready` and defer endpoint readiness to `call()`. ADR-0010 is the explicit exception for `SecurityPolicyService`: pre-pipeline authorization faults must surface before data-plane processing.

| Processor | poll_ready behavior | Status | Rationale / source |
|---|---|---|---|
| `MulticastService` | `Ready(Ok(()))` unconditional | migrated | Per-endpoint readiness happens inside fan-out logic; `src/multicast.rs:48-53`. |
| `ErrorHandlerService` | preserves `Pending`, maps readiness `Err` to `Ok(())` | migrated | Deprecated compatibility shell still follows ADR-0019; `src/error_handler.rs:472-482`. |
| `AggregatorService` | `Ready(Ok(()))` unconditional | migrated | Aggregation state is owned by `call()`; `src/aggregator.rs:188-190`. |
| `RecipientListService` | `Ready(Ok(()))` unconditional | migrated | Dynamic recipients resolve and check readiness in `call()`; `src/recipient_list.rs:43-45`. |
| `WireTapService` | delegates to tap endpoint | pending-fix | Fire-and-forget tap errors must not block the main route; `src/wire_tap.rs:60-62`. |
| `LoadBalancerService` | polls all endpoints and returns first `Err` | pending-fix | Failover/selection must happen in `call()`; `src/load_balancer.rs:40-49`. |
| `SplitterService` | delegates to sub-pipeline | pending-fix | Fragment processing already checks readiness per fragment in `call()`; `src/splitter.rs:75-77`, `src/splitter.rs:145-153`. |
| `StreamingSplitterService` | delegates to sub-pipeline | pending-fix | Streaming fragment processing checks readiness per fragment in `call()`; `src/streaming_splitter.rs:54-56`, `src/streaming_splitter.rs:97-102`. |
| `SecurityPolicyService` | delegates to inner service | excluded | Pre-pipeline authorization gate; authz faults are outside normal EIP recovery per ADR-0010; `src/security_policy_layer.rs:58-60`. |

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
`fragment_stream_exchange` strips `Content-Length` and `Content-Type` from fragment headers — these belong to the parent stream body, not individual fragments. See `stream_codec.rs:70-74`.

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

## ADR-0012 log-policy sites

All 5 sites in this crate are category **(a) handler-owned** — EIP processor failures that occur
INSIDE the pipeline where the route ErrorHandler owns ERROR responsibility. These have been
downgraded from `error!` to `warn!`.

| File | Line | Category | Annotation |
|------|------|----------|------------|
| `aggregator.rs` | 119 | (a) handler-owned | `// log-policy: handler-owned` — force_complete_all failed |
| `aggregator.rs` | 463 | (a) handler-owned | `// log-policy: handler-owned` — timeout task failed |
| `log.rs` | 64 | (a) handler-owned | `// log-policy: handler-owned` — LogProcessor::call default Error level |
| `log.rs` | 113 | (a) handler-owned | `// log-policy: handler-owned` — DynamicLog::call default Error level |
| `wire_tap.rs` | 87 | (a) handler-owned | `// log-policy: handler-owned` — WireTap processing error |

## Metrics

Metrics instrumentation is not yet wired for most processors. See TODO(PROC-004) in
`camel-processor/src/log.rs` for the broader instrumentation gap.
