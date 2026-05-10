# rag-file — Live Document Q&A

Drop `.txt` files into `./docs/` to index them, then ask questions via HTTP.

## Prerequisites

- **Ollama** running on `localhost:11434` with `embeddinggemma` and `qwen3.5:4b` models pulled
- **Qdrant** running on `localhost:6334` (or use docker-compose below)

## Quick Start

```bash
# Start Qdrant
docker compose up -d

# Pull Ollama models (if not already)
ollama pull embeddinggemma
ollama pull qwen3.5:4b

# Run the example
cargo run -p rag-file
```

## Usage

1. Drop `.txt` files into the `docs/` directory — they will be automatically indexed
2. Ask questions via HTTP POST:

```bash
curl -d "What is rust-camel?" http://localhost:8080/ask
```

Returns a JSON answer from the LLM based on your indexed documents.

## How it works

- **Route 1 (Ingest):** Polls `./docs/` every 2 seconds for new `.txt` files → embeds content → upserts into Qdrant collection `rag-file`. Files are NOT deleted (`noop=true`), but already-seen files are skipped by the idempotent consumer.
- **Route 2 (Query):** HTTP POST `/ask` — the request body is the question text, which is embedded and searched against Qdrant (top 3 results), then the LLM synthesizes an answer from the retrieved context.
- A `docs/sample.txt` is included for immediate testing on first run.
