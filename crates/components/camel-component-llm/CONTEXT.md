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

## Breaking changes (0.x)

- `LlmGlobalConfig.timeout_secs`: `u64` → `Option<u64>` (use `None` for no timeout)
- `OpenaiProviderConfig`/`OllamaProviderConfig`: new `network_retry` field (`Option<NetworkRetryPolicy>`)
- `LlmProducer::new`: expanded signature — added `semaphore: Option<Arc<Semaphore>>`, `timeout: Option<Duration>`, `retry: Option<NetworkRetryPolicy>` parameters

## Log-level policy

| Site | Level | Rationale |
|------|-------|-----------|
| Producer receives error from provider | `warn!` | Operational signal; route handler owns `error!` |
| ProviderFactory fails to construct provider | `error!` | Startup failure; no route handler running |
| Stream finished with usage | `info!` | Observability for token consumption |
| Config validation error | `error!` | Startup, fatal |
| Retry attempt fired | `warn!` | Transient error, operational signal |
| Timeout fired | `warn!` | Total deadline elapse, operational signal |
