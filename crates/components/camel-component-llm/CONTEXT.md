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

## Log-level policy

| Site | Level | Rationale |
|------|-------|-----------|
| Producer receives error from provider | `warn!` | Operational signal; route handler owns `error!` |
| ProviderFactory fails to construct provider | `error!` | Startup failure; no route handler running |
| Stream finished with usage | `info!` | Observability for token consumption |
| Config validation error | `error!` | Startup, fatal |
