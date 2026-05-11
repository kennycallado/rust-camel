# Webmaster Agent POC

Minimal vertical slice for a **Camel Webmaster Agent** that:

- builds a structured `SystemSnapshot` from a demo route file,
- runs heuristic maintenance analysis,
- prints structured JSON proposals.

## Run

```bash
cargo run -p webmaster-agent
```

The example does not require OpenAI/Ollama/Qdrant and should print multiple maintenance proposals in JSON.
