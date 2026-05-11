# Webmaster Agent POC (Phase 0)

Minimal observer-only slice for a **Camel Webmaster Agent** that:

- parses real rust-camel DSL YAML (`camel_dsl::parse_yaml_to_declarative`),
- builds a structured `SystemSnapshot` from declarative routes,
- runs heuristic maintenance analysis,
- prints structured JSON proposals.

This phase does not execute routes or integrate an `agent:` runtime component yet.

## Run

```bash
cargo run -p webmaster-agent
```

The example does not require OpenAI/Ollama/Qdrant and should print multiple maintenance proposals in JSON.
