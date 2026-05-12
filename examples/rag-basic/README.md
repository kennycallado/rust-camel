# RAG Basic — Minimal Retrieval-Augmented Generation

The simplest RAG demo: index one document, ask one question, get an AI-generated answer.

## Prerequisites

- [Ollama](https://ollama.ai) running at `localhost:11434` with models:
  - `embeddinggemma` (embeddings)
  - `qwen3.5:4b` (LLM)
- [Qdrant](https://qdrant.tech) running at `localhost:6334`

## Run

```bash
docker-compose up -d
cargo run -p rag-basic
```

## Expected Output

```
RAG Basic: Minimal RAG Demo
===========================
Indexes one document and queries it.
Demonstrates: embedding → vector store → prompt_template → LLM

Requires: Ollama (embeddinggemma + qwen3.5:4b) + Qdrant
Press Ctrl+C to stop.

 INFO rag_basic: Ingested sample document
 INFO rag_basic: RAG answer: AcmeCorp offers three support plans: Starter at $29/month, Professional at $79/month, and Enterprise with custom pricing. All plans include email support, while Professional and Enterprise also include phone and chat support.
```

For a more complete knowledge-base demo, see `rag-file/`.
