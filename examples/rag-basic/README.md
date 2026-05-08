# rag-basic

Basic Retrieval-Augmented Generation (RAG) example using rust-camel.

## Architecture

```
timer:tick (one-shot at 60s)
  ↓ set_body (sample document)
  ↓ embedding:create  (embeddinggemma)
  ↓ vector:upsert     (Qdrant)

timer:tick (one-shot at 65s)
  ↓ set_body (query)
  ↓ embedding:create  (embeddinggemma)
  ↓ vector:search     (Qdrant top-3)
  ↓ ai_extract        (qwen3.5:4b)
  → JSON answer in header rag_result
```

## Prerequisites

```bash
docker compose up -d
docker exec -it rag-basic-ollama-1 ollama pull embeddinggemma
docker exec -it rag-basic-ollama-1 ollama pull qwen3.5:4b
```

## Run

```bash
cargo run -p rag-basic
```
