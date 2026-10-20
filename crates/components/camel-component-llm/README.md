# camel-component-llm

LLM integration component for [rust-camel](https://crates.io/crates/camel-api).

## Features

- **Chat (streaming + materialized)** — Stream tokens per-event or collect into a single response body
- **Embeddings** — Generate vector embeddings with the genai-backed OpenAI and Ollama providers. Mock also embeds for tests; generic Anthropic and Gemini entries are chat-only
- **Generic native chat** — Anthropic and Gemini through one `type = "generic"` shape with an explicit service selector
- **Tool calling** — Pass tool definitions via header; component emits tool-call intents (route owns dispatch)
- **Multi-turn conversations** — Full conversation history with system, user, assistant, and tool roles
- **Response cache** — Materialized-only, single-flight via dashmap + `tokio::sync::watch`
- **Cost observability** — Config-driven pricing table; estimated cost emitted as header + `tracing::info!`
- **Retry** — Manual loop honoring provider `retry_after` over exponential backoff (ADR-0021)
- **Concurrency control** — Producer semaphore caps in-flight requests; permit released during backoff

## Quick Start

Add to `Camel.toml` under the `[components.llm]` table that the bundle loader reads:

```toml
[components.llm]
default_provider = "my-openai"

[components.llm.providers.my-openai]
type = "openai"
api_key = "${env:OPENAI_API_KEY}"
default_model = "gpt-4o"

[components.llm.providers.local]
type = "ollama"
base_url = "http://localhost:11434"
default_model = "llama3"
```

`Camel.toml` resolves `${env:OPENAI_API_KEY}` in string leaves before the `[components.llm]` table is deserialized. Direct deserialization through `LlmBundle::from_toml` does not expand placeholders. This is application-level configuration interpolation, not SDK credential discovery.

For local Ollama, set `allow_internal = true` to permit HTTP + loopback addresses:

```toml
[components.llm]
allow_internal = true
```

### Generic native providers

The `generic-providers` feature enables native Anthropic and Gemini chat. The `all-providers` feature includes it. The default build contains Mock only.

```toml
[components.llm]
default_provider = "anthropic-native"

[components.llm.providers.anthropic-native]
type = "generic"
service = "anthropic"
api_key = "replace-with-anthropic-api-key"
default_model = "claude-sonnet-4-0"

[components.llm.providers.gemini-native]
type = "generic"
service = "gemini"
api_key = "replace-with-gemini-api-key"
default_model = "gemini-2.0-flash"
```

A generic entry requires `type = "generic"`, `service`, `api_key`, and `default_model`. It accepts the shared policy fields `base_url`, `timeout_secs`, `max_concurrency`, `network_retry`, `pricing`, `cache_ttl_secs`, and `cache_max_entries`. The `service` selector is configuration data. It is not the Tower `Service` trait and not the Services architecture tier.

Credentials and models are explicit. The component does not read environment variables and does not infer a service from a model name. An application can resolve `${...}` before it builds the TOML value. Direct deserialization does not expand those strings. A request `model` parameter overrides `default_model` without changing the service.

Default endpoints are `https://api.anthropic.com/v1/` for Anthropic and `https://generativelanguage.googleapis.com/v1beta/` for Gemini. A `base_url` override replaces the default, and Camel adds a trailing slash when it is absent. Anthropic authentication uses the `x-api-key` header. Gemini authentication uses the `x-goog-api-key` header. If an Anthropic request omits `max_tokens`, genai chooses the default, not Camel.

Generic providers are chat-only. `supports_embed` returns `false` and `llm:embed` returns `UnsupportedCapability` before a request exists. Without the feature, configuration still parses but construction returns `UnsupportedCapability` naming `generic-providers`.

Gemini reports request throttling and daily or project quota exhaustion as HTTP 429 `RESOURCE_EXHAUSTED`. Both cases map to retryable `RateLimit` with no retry-after value. Retries follow the configured `network_retry` policy. `max_attempts = 0` is unlimited unless `max_attempts_absolute` sets a cap. `QuotaExceeded` is a separate OpenAI-style `insufficient_quota` mapping. It is not retryable. Not every vendor quota response becomes `QuotaExceeded`.

The LLM guide has full examples and operational notes: [`docs/src/components/llm.md`](../../../docs/src/components/llm.md).

Or use `LlmBundle` programmatically:

```rust
use camel_component_llm::LlmBundle;
ctx.add_bundle(LlmBundle::from_toml(config))?;
```

## URI Scheme

```
llm:{operation}?provider={name}&model={model}&temperature={n}&max_tokens={n}&stream={bool}&system_prompt={text}&timeout_secs={n}
```

### Operations

| Operation | Description |
|-----------|-------------|
| `llm:chat`  | Chat completion. Defaults to streaming (`stream=true`) |
| `llm:embed` | Text embedding. Always materialized |

### URI Parameters

| Parameter        | Default      | Description |
|------------------|--------------|-------------|
| `provider`       | _(required)_ | Provider name from config |
| `model`          | provider default | Override model |
| `temperature`    | provider default | Sampling temperature |
| `max_tokens`     | provider default | Max output tokens |
| `stream`         | `true`       | `true` = streaming, `false` = materialized |
| `system_prompt`  | —            | System prompt override |
| `timeout_secs`   | provider default | Activity timeout (streaming) or total deadline (materialized) |

### Exchange Headers

See [`src/headers.rs`](src/headers.rs) for the full list. Key headers:

| Header | Direction | Description |
|--------|-----------|-------------|
| `CamelLlmProvider` | Input/Output | Provider name |
| `CamelLlmModel` | Input/Output | Model name |
| `CamelLlmStream` | Input | `true` = streaming mode |
| `CamelLlmSystemPrompt` | Input | Override system prompt |
| `CamelLlmTools` | Input | Tool definitions (JSON array) |
| `CamelLlmToolChoice` | Input | Tool selection strategy |
| `CamelLlmToolCalls` | Output | Tool call intents emitted by the model |
| `CamelLlmTokensIn` | Output | Input token count (materialized only) |
| `CamelLlmTokensOut` | Output | Output token count (materialized only) |
| `CamelLlmFinishReason` | Output | Finish reason (`stop`, `length`, `tool_calls`) |
| `CamelLlmUsageAvailable` | Output | Whether usage/headers are populated |
| `CamelLlmEstimatedCostUsd` | Output | Estimated cost in USD (materialized only) |
| `CamelLlmMessages` | Input | Multi-turn conversation history |
| `CamelLlmText` | Output | Response text |

## Provider Feature Flags

| Provider | Cargo feature | Requires genai |
|----------|---------------|----------------|
| Mock     | `mock` (default) | No |
| OpenAI   | `openai`     | Yes (genai-backed) |
| Ollama   | `ollama`     | Yes (genai-backed) |
| Anthropic | `generic-providers` | Yes (genai-backed) |
| Gemini    | `generic-providers` | Yes (genai-backed) |

Enable multiple: `--features "openai,ollama,generic-providers"` or use `all-providers`.

## Architecture Decisions

- [ADR-0020](https://github.com/kenny/rust-camel/blob/main/docs/adr/0020-llm-component-provider-adapter-boundary.md) — Project-owned `LlmProvider` trait isolates genai SDK behind `provider/genai_adapter.rs`
- [ADR-0021](https://github.com/kenny/rust-camel/blob/main/docs/adr/0021-llm-retry-retry-after-manual-loop.md) — Manual retry loop honors `retry_after` over exponential backoff

## Example

See [`examples/llm-example/`](https://github.com/kenny/rust-camel/tree/main/examples/llm-example) for a working chat example.

## License

Licensed under either of Apache License, Version 2.0 or MIT License at your option.
