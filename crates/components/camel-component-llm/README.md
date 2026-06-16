# camel-component-llm

LLM integration component for [rust-camel](https://crates.io/crates/camel-api).

## Features

- **Chat (streaming + materialized)** — Stream tokens per-event or collect into a single response body
- **Embeddings** — Generate vector embeddings from text
- **Tool calling** — Pass tool definitions via header; component emits tool-call intents (route owns dispatch)
- **Multi-turn conversations** — Full conversation history with system, user, assistant, and tool roles
- **Response cache** — Materialized-only, single-flight via dashmap + `tokio::sync::watch`
- **Cost observability** — Config-driven pricing table; estimated cost emitted as header + `tracing::info!`
- **Retry** — Manual loop honoring provider `retry_after` over exponential backoff (ADR-0021)
- **Concurrency control** — Producer semaphore caps in-flight requests; permit released during backoff

## Quick Start

Add to `Camel.toml`:

```toml
[llm]
[[llm.providers]]
name = "my-openai"
adapter = "openai"
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"

[[llm.providers]]
name = "local"
adapter = "ollama"
model = "llama3"
base_url = "http://localhost:11434/v1"
```

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

| Provider | Cargo feature | Requires siumai |
|----------|---------------|-----------------|
| Mock     | `mock` (default) | No |
| OpenAI   | `openai`     | Yes |
| Ollama   | `ollama`     | Yes |

Enable multiple: `--features "openai,ollama"` or use `all-providers`.

## Architecture Decisions

- [ADR-0020](https://github.com/kenny/rust-camel/blob/main/docs/adr/0020-llm-component-provider-adapter-boundary.md) — Project-owned `LlmProvider` trait isolates siumai SDK behind two files
- [ADR-0021](https://github.com/kenny/rust-camel/blob/main/docs/adr/0021-llm-retry-retry-after-manual-loop.md) — Manual retry loop honors `retry_after` over exponential backoff

## Example

See [`examples/llm-example/`](https://github.com/kenny/rust-camel/tree/main/examples/llm-example) for a working chat example.

## License

Licensed under either of Apache License, Version 2.0 or MIT License at your option.
