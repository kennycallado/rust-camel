# LLM

The LLM component sends chat, embedding, and tool-calling requests to language model providers. It streams tokens or materializes full responses. The component ships OpenAI, Ollama, Mock, and generic Anthropic and Gemini providers. The component is producer-only. It has no Consumer in the current release.

The llm-example registers a Mock provider and runs three routes. The materialized route below sends a prompt, collects the response, and logs it with headers:

```rust,ignore
let route = RouteBuilder::from("timer:tick?period=3000")
    .route_id("llm-materialized")
    .set_body("Explain Tower middleware in one sentence.")
    .to("llm:chat?provider=openai-prod&model=gpt-4o&stream=false")
    .to("log:info?showBody=true&showHeaders=true")
    .build()?;
ctx.add_route_definition(route).await?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: llm-materialized
    from: "timer:tick?period=3000"
    steps:
      - set_body: "Explain Tower middleware in one sentence."
      - to: "llm:chat?provider=openai-prod&model=gpt-4o&stream=false"
      - to: "log:info?showBody=true&showHeaders=true"
```

</details>

## URI

```text
llm:<operation>?provider=<name>[&model=<model>][&temperature=<n>][&max_tokens=<n>][&stream=<bool>][&system_prompt=<text>]
```

| Parameter | Default | Description |
| --- | --- | --- |
| `provider` | global default | Provider name from `Camel.toml` |
| `model` | provider default | Override model |
| `temperature` | provider default | Sampling temperature |
| `max_tokens` | provider default | Max output tokens |
| `stream` | `true` | `true` streams, `false` materializes |
| `system_prompt` | — | System prompt override |

`timeout_secs` is not a URI parameter. Configure it in the provider section of `Camel.toml` (or the global LLM config). It sets the activity timeout for streaming mode and the total deadline for materialized mode.

## Operations

`llm:chat` runs a chat completion. `llm:embed` generates a vector embedding. The operation picks which `LlmProvider` method the producer calls. Generic Anthropic and Gemini providers are chat-only, so they reject `llm:embed`.

## Producer

`llm:chat?provider=openai-prod&model=gpt-4o` sends the Exchange body as a user message. The `stream` parameter selects streaming or materialized mode.

Streaming mode is the default. The producer returns `Body::Stream(StreamBody)`. Each `ChatEvent::Delta` carries a token or text fragment. Usage metadata goes to `tracing::info!`. Usage metadata does not go to Exchange headers. The header `CamelLlmUsageAvailable` is `false` at Exchange return time.

Materialized mode (`stream=false`) collects every `ChatEvent` into a single `Body::Text`. The producer writes token counts to `CamelLlmTokensIn` and `CamelLlmTokensOut`. It writes the finish reason to `CamelLlmFinishReason`. The `CamelLlmUsageAvailable` header is `true`.

Tool calls flow through `CamelLlmTools` (input) and `CamelLlmToolCalls` (output). The component emits tool-call intent through `ChatEvent::ToolCall`. The component never executes tools. The route owns dispatch.

## Cost and cache

Cost observability tracks spending per request. A config-driven `PricingTable` maps input and output tokens to USD rates. The producer computes cost from final usage. It writes cost to `CamelLlmEstimatedCostUsd` in materialized mode. It logs cost at `info!` in both modes. Missing pricing means no cost header and no failure.

The response cache deduplicates materialized requests. It sits at the producer level, not the provider level. A single-flight mechanism built on `dashmap` and `tokio::sync::watch` collapses concurrent lookups. Entries expire by TTL and evict by LRU. The cache stores usage, not cost. The cache lookup runs before the semaphore, retry, and timeout layers.

## Providers

The Mock provider ships in the default build. OpenAI, Ollama, Anthropic, and Gemini require cargo features. Use `--features openai`, `--features ollama`, or `--features generic-providers`. The `all-providers` feature enables all four. The `GenaiProvider` adapter isolates the genai SDK. If genai breaks, only `provider/genai_adapter.rs` changes (ADR-0020).

### Generic native providers

A generic provider selects a native protocol through `service`. The supported selectors are `anthropic` and `gemini`. The selector is configuration data. It is not the Tower `Service` trait and not the Services architecture tier.

The generic provider requires `type = "generic"`, `service`, `api_key`, and `default_model`. It accepts the shared policy fields `base_url`, `timeout_secs`, `max_concurrency`, `network_retry`, `pricing`, `cache_ttl_secs`, and `cache_max_entries`. These fields use the existing provider wrappers and precedence rules.

Credentials are explicit. The component does not read `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, or any other environment variable. An application can resolve an environment variable before it builds the TOML value. Direct TOML deserialization does not expand `${...}`. Models are explicit too. Camel does not infer a service from a model name.

The examples show the contents of the `llm` configuration value. In `Camel.toml`, place that value under `[components.llm]` and prefix each table with `components.llm.` (for example, `[components.llm.providers.anthropic-native]`).

### Anthropic generic provider

```toml
default_provider = "anthropic-native"

[providers.anthropic-native]
type = "generic"
service = "anthropic"
api_key = "replace-with-anthropic-api-key"
default_model = "claude-sonnet-4-0"

# Optional overrides and shared policy:
# base_url = "https://api.anthropic.com/v1"
# timeout_secs = 30
# max_concurrency = 4
# cache_ttl_secs = 60
# cache_max_entries = 256
```

The default endpoint is `https://api.anthropic.com/v1/`. An explicit `base_url` replaces it. Camel adds a trailing slash when the override has none. Authentication uses the configured key. The SDK sends it in the `x-api-key` header.

A request `model` URI parameter overrides `default_model` for that request. The selected service does not change. If the request omits `max_tokens`, genai chooses the default. Camel does not set or reimplement that default.

### Gemini generic provider

```toml
default_provider = "gemini-native"

[providers.gemini-native]
type = "generic"
service = "gemini"
api_key = "replace-with-gemini-api-key"
default_model = "gemini-2.0-flash"

# Optional overrides and shared policy:
# base_url = "https://generativelanguage.googleapis.com/v1beta"
# timeout_secs = 30
# max_concurrency = 4
```

The default endpoint is `https://generativelanguage.googleapis.com/v1beta/`. Override normalization and request model precedence match Anthropic. Authentication uses the configured key. The SDK sends it in the `x-goog-api-key` header.

### Generic provider availability

`generic-providers` enables both native services and uses the optional genai dependency. `all-providers` includes it. The default feature set contains Mock only, so a generic entry in a default build parses but fails at construction. The error is `UnsupportedCapability` and names `generic-providers`. The configuration types stay available in every build.

Generic Anthropic and Gemini providers support chat only. `supports_embed` returns `false`, and `llm:embed` fails with `UnsupportedCapability` before a client or network request exists. OpenAI and Ollama embeddings do not change.

Generic providers keep the existing factory URL checks and DNS pinning. A custom `base_url` must satisfy the current SSRF policy. Loopback and private addresses need `allow_internal = true`, as for other providers.

### Gemini quota exhaustion and retry

Gemini reports both request throttling and daily or project quota exhaustion as HTTP 429 with `RESOURCE_EXHAUSTED`. The inherited status mapping turns both cases into `RateLimit` with no retry-after value. `RateLimit` is retryable. Retries use the configured `network_retry` policy. `max_attempts = 0` means unlimited retries unless `max_attempts_absolute` sets a cap. This policy does not make Gemini errors bounded when the operator removes both limits.

`QuotaExceeded` is a separate OpenAI-style `insufficient_quota` mapping. It is not retryable and fails fast. Not every vendor quota response becomes `QuotaExceeded`. The Gemini `RESOURCE_EXHAUSTED` response above stays `RateLimit`.

## Retry and timeout

The manual retry loop honors provider `retry_after` over exponential backoff (ADR-0021). The loop only runs in materialized mode. Once content starts streaming, the loop stops.

The streaming `timeout_secs` covers each `stream.next()` call. A deadline lapse yields a `Timeout` error. The materialized total deadline covers the whole request.

## Error handling

`LlmError` maps to three `CamelError` variants: `Unauthenticated`, `Io`, and `ProcessorError`. The mapping is lossy. Downstream handlers that need typed LLM errors should use `CamelError::ProcessorErrorWithSource`. The component has no Consumer. It rejects `create_consumer` with `CamelError::InvalidUri`.

The producer logs provider errors at `warn!`. The route `ErrorHandler` owns the `error!` level. Stream finish and tool dispatch log at `info!`. Cache hit and miss log at `debug!`.

**Reference**: [LLM crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-llm/CONTEXT.md), [ADR-0020](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0020-llm-component-provider-adapter-boundary.md), [ADR-0021](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0021-llm-retry-retry-after-manual-loop.md). Example source: [`examples/llm-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/llm-example).
