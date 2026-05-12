# RAG Basic

Minimal RAG pipeline: embed -> store -> search -> answer.

## Prerequisites

- [Ollama](https://ollama.ai) running at `localhost:11434` with `embeddinggemma` model
- [Qdrant](https://qdrant.tech) running at `localhost:6334`

## Run

```bash
cargo run -p rag-basic
```

## Expected Output

Route 1 ingests a sample doc at t+60s. Route 2 queries at t+75s and prints the RAG answer.
