# AI Ticket Router

Classifies support tickets into categories using an LLM.

## Prerequisites

- [Ollama](https://ollama.ai) running at `localhost:11434`
- Model pulled: `ollama pull qwen3.5:4b`

## Run

```bash
cargo run -p ai-ticket-router
```

## Expected Output

Two sample tickets are classified after ~5 seconds:

```
category=billing
category=technical
```

Headers also include `CamelAiOperation`, `CamelAiLatencyMs`, `CamelAiTotalTokens`.
