# camel-component-llm

## Language

**LlmProvider**:
Trait abstraction over LLM backends (OpenAI, Ollama, Mock). Each provider implements `chat_stream()` and `embed()`. Owned by `LlmComponent` as `Arc<dyn LlmProvider>` in a `ProviderMap`.
_Avoid_: client, connection, SDK wrapper

**LlmComponent**:
The `Component` impl (scheme `"llm"`) that owns the `ProviderMap` and creates `LlmEndpoint`s from URIs.
_Avoid_: registry, manager

**ChatEvent**:
Unit of a chat stream. Either a `Delta` (partial text) or `Finished` (terminal metadata with usage). Produced by `LlmProvider::chat_stream()`.
_Avoid_: chunk, token, message (use ChatMessage for request input)

**LlmOperation**:
The URI path component (`chat` or `embed`). Determines which provider trait method is called.
_Avoid_: mode, action, verb

**ProviderMap**:
`HashMap<String, Arc<dyn LlmProvider>>` owned by `LlmComponent`. Resolved by name from config. Not a global registry — safe for tests, hot-reload, multi-context.
_Avoid_: registry, pool, factory (use provider_factory for the builder function)

**SiumaiProvider**:
The adapter that bridges `LlmProvider` to the siumai SDK. Lives in `provider/siumai_adapter.rs`. If siumai breaks, only this file changes.
_Avoid_: OpenAiClient, OllamaClient (those are siumai types, never exposed)

**Materialized mode**:
When `stream=false`, the producer collects all `ChatEvent`s into a single `Body::Text` with complete usage headers (`CamelLlmTokensIn`, `CamelLlmTokensOut`).
_Avoid_: sync mode, blocking mode

**Streaming mode**:
Default. Producer returns `Body::Stream(StreamBody)`. Usage metadata goes to metrics/tracing, NOT Exchange headers (race condition).
_Avoid_: async mode (ambiguous with async runtime)

**Usage availability**:
Whether token counts are knowable at Exchange return time. Streaming: `false` (arrives at stream end → metrics). Materialized: `true` (in headers). Stored in `CamelLlmUsageAvailable` header.
_Avoid_: token tracking, billing

**Activity timeout**:
Streaming `timeout_secs` covers every `stream.next()`. If no event arrives within the deadline, the stream yields a `Timeout` error. Contrasts with materialized total-deadline timeout.
_Avoid_: connection timeout, idle timeout

**Producer semaphore**:
Wraps provider work to enforce `max_concurrency`. For streaming, the permit is held by `PermitStream` inside the returned `Body::Stream` and released when the stream is consumed/dropped.
_Avoid_: connection pool, rate limiter

**Retry-after honoring**:
The manual retry loop (ADR-0021) uses `RateLimit.retry_after` over exponential backoff when present. Materialized-only; no retry after content-start.
_Avoid_: backoff override, provider delay

**ToolDefinition / ToolChoice / ToolCall**:
Tools the model may call, passed via `CamelLlmTools` header (JSON array). Component emits tool-call intent via `ChatEvent::ToolCall` — it NEVER executes tools (the route owns dispatch). Multi-turn: `ChatRole::Tool { tool_call_id }` carries prior tool results; assistant messages carry `tool_calls` for conversation history.
_Avoid_: tool executor, tool runner

**ProducerCache**:
Materialized-only response cache at the producer level (not a provider decorator). Single-flight via `tokio::sync::watch` — leader detected by dashmap `Entry::Vacant`; waiters hold zero permits. Lookup before semaphore/retry/timeout. TTL-based; LRU eviction deferred. Stores usage (not cost).
_Avoid_: response cache, query cache

**PricingTable / cost observability**:
Config-driven pricing (input/output per 1k tokens). Producer computes cost from final usage and emits `CamelLlmEstimatedCostUsd` header (materialized) + `tracing::info!` (both modes). Missing pricing → no cost, no failure.
_Avoid_: billing, accounting

## Breaking changes (0.x)

- `LlmGlobalConfig.timeout_secs`: `u64` → `Option<u64>` (use `None` for no timeout)
- `OpenaiProviderConfig`/`OllamaProviderConfig`: new `network_retry` field (`Option<NetworkRetryPolicy>`)
- `LlmProducer::new`: expanded signature — added `semaphore: Option<Arc<Semaphore>>`, `timeout: Option<Duration>`, `retry: Option<NetworkRetryPolicy>` parameters
- `ChatRole`: dropped `Copy` (Tool variant holds a `String`); added `#[non_exhaustive]`
- `ChatEvent`: added `#[non_exhaustive]`; new `ToolCall` variant
- `ChatRequest`: new fields `tools: Vec<ToolDefinition>`, `tool_choice: Option<ToolChoice>`
- `ChatMessage`: new field `tool_calls: Option<Vec<EmittedToolCall>>`
- `build_chat_request`: return type changed from `ChatRequest` to `Result<ChatRequest, LlmError>`
- `LlmProducer::new`: expanded to 9 params (added `pricing: Option<Arc<PricingTable>>`, `cache: Option<Arc<ProducerCache>>`)

## Log-level policy

| Site | Level | Rationale |
|------|-------|-----------|
| Producer receives error from provider | `warn!` | Operational signal; route handler owns `error!` |
| ProviderFactory fails to construct provider | `error!` | Startup failure; no route handler running |
| Stream finished with usage | `info!` | Observability for token consumption |
| Config validation error | `error!` | Startup, fatal |
| Retry attempt fired | `warn!` | Transient error, operational signal |
| Timeout fired | `warn!` | Total deadline elapse, operational signal |
| Tool call emitted (streaming) | `info!` | Observability for tool dispatch |
| Tool call collected (materialized) | `info!` | Observability for tool dispatch |
| Cost computed | `info!` | Cost observability per request |
| Cache hit | `debug!` | Operational signal, not route-relevant |
| Cache miss | `debug!` | Operational signal, not route-relevant |
| Tool call delta without prior start | `warn!` | Provider protocol violation |
