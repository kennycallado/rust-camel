# LLM Example

Demonstrates the LLM component for rust-camel with three chat modes.

## What it demonstrates

- **Route 1 — Materialized Chat**: Non-streaming LLM chat (`stream=false`). The full response arrives as `Body::Text` with token usage headers.
- **Route 2 — Streaming Chat**: Streaming LLM chat (`stream=true`). The response is a `Body::Stream` that must be materialized (via `stream_cache_default()`) before downstream processing.
- **Route 3 — Tool Calling**: Sets the `CamelLlmTools` header with tool definitions to show the tool-calling API surface.

All routes use the **mock** provider (echo mode) — no external API key required.

## Run

```bash
cargo run -p llm-example
```

Press Ctrl+C to stop.

## Expected output

Each route fires once, then the program waits for Ctrl+C:

```
=== Route 1: Materialized Chat (stream=false) ===
  timer -> llm:chat?stream=false -> log

=== Route 2: Streaming Chat (stream=true) ===
  timer -> llm:chat?stream=true -> stream_cache -> log

=== Route 3: Tool Calling (CamelLlmTools header) ===
  timer -> set_header CamelLlmTools -> llm:chat?stream=false -> log

Starting LLM example... Press Ctrl+C to stop.
```

Route 1 and 3 log the echoed prompt text with LLM headers (`CamelLlmProvider`, `CamelLlmTokensIn`, `CamelLlmFinishReason`, etc.).
Route 2 logs the same (streaming response materialized via stream cache).

## Swap mock → OpenAI / Ollama

1. **Enable the provider feature** in `Cargo.toml`:

   ```toml
   camel-component-llm = { workspace = true, features = ["openai"] }
   ```

   or:

   ```toml
   camel-component-llm = { workspace = true, features = ["ollama"] }
   ```

2. **Update the bundle config** in `src/main.rs`:

   For OpenAI:

   ```toml
   [providers.my-openai]
   type = "openai"
   api_key = "sk-..."           # or $OPENAI_API_KEY
   default_model = "gpt-4o"
   ```

   For Ollama:

   ```toml
   [providers.my-ollama]
   type = "ollama"
   base_url = "http://localhost:11434"
   default_model = "llama3"
   ```

3. **Update the URIs** to reference the real provider name, e.g.:

   ```
   llm:chat?provider=my-openai&model=gpt-4o&stream=false
   ```
