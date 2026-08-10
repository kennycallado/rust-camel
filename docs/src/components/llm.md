# LLM

The LLM component sends chat, embedding, and tool-calling requests to language model providers. It streams tokens or materializes full responses. Three providers ship: OpenAI, Ollama, and Mock. The component is producer-only. It has no Consumer in the current release.

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
llm:<operation>?provider=<name>[&model=<model>][&temperature=<n>][&max_tokens=<n>][&stream=<bool>][&system_prompt=<text>][&timeout_secs=<n>]
```

| Parameter | Default | Description |
| --- | --- | --- |
| `provider` | global default | Provider name from `Camel.toml` |
| `model` | provider default | Override model |
| `temperature` | provider default | Sampling temperature |
| `max_tokens` | provider default | Max output tokens |
| `stream` | `true` | `true` streams, `false` materializes |
| `system_prompt` | — | System prompt override |
| `timeout_secs` | provider default | Activity timeout (streaming) or total deadline (materialized) |

## Operations

`llm:chat` runs a chat completion. `llm:embed` generates a vector embedding. The operation picks which `LlmProvider` method the producer calls.

## Producer

`llm:chat?provider=openai-prod&model=gpt-4o` sends the Exchange body as a user message. The `stream` parameter selects streaming or materialized mode.

Streaming mode is the default. The producer returns `Body::Stream(StreamBody)`. Each `ChatEvent::Delta` carries a token or text fragment. Usage metadata goes to `tracing::info!`. Usage metadata does not go to Exchange headers. The header `CamelLlmUsageAvailable` is `false` at Exchange return time.

Materialized mode (`stream=false`) collects every `ChatEvent` into a single `Body::Text`. The producer writes token counts to `CamelLlmTokensIn` and `CamelLlmTokensOut`. It writes the finish reason to `CamelLlmFinishReason`. The `CamelLlmUsageAvailable` header is `true`.

Tool calls flow through `CamelLlmTools` (input) and `CamelLlmToolCalls` (output). The component emits tool-call intent through `ChatEvent::ToolCall`. The component never executes tools. The route owns dispatch.

## Cost and cache

Cost observability tracks spending per request. A config-driven `PricingTable` maps input and output tokens to USD rates. The producer computes cost from final usage. It writes cost to `CamelLlmEstimatedCostUsd` in materialized mode. It logs cost at `info!` in both modes. Missing pricing means no cost header and no failure.

The response cache deduplicates materialized requests. It sits at the producer level, not the provider level. A single-flight mechanism built on `dashmap` and `tokio::sync::watch` collapses concurrent lookups. Entries expire by TTL and evict by LRU. The cache stores usage, not cost. The cache lookup runs before the semaphore, retry, and timeout layers.

## Providers

The Mock provider ships in the default build. OpenAI and Ollama require cargo features (`--features openai` and `--features ollama`). The `SiumaiProvider` adapter isolates the siumai SDK. If siumai breaks, only `provider/siumai_adapter.rs` changes (ADR-0020).

## Retry and timeout

The manual retry loop honors provider `retry_after` over exponential backoff (ADR-0021). The loop only runs in materialized mode. Once content starts streaming, the loop stops.

The streaming `timeout_secs` covers each `stream.next()` call. A deadline lapse yields a `Timeout` error. The materialized total deadline covers the whole request.

## Error handling

`LlmError` maps to three `CamelError` variants: `Unauthenticated`, `Io`, and `ProcessorError`. The mapping is lossy. Downstream handlers that need typed LLM errors should use `CamelError::ProcessorErrorWithSource`. The component has no Consumer. It rejects `create_consumer` with `CamelError::InvalidUri`.

The producer logs provider errors at `warn!`. The route `ErrorHandler` owns the `error!` level. Stream finish and tool dispatch log at `info!`. Cache hit and miss log at `debug!`.

**Reference**: [LLM crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-llm/CONTEXT.md), [ADR-0020](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0020-llm-component-provider-adapter-boundary.md), [ADR-0021](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0021-llm-retry-retry-after-manual-loop.md). Example source: [`examples/llm-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/llm-example).
