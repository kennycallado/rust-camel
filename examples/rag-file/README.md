# RAG File

File-based RAG: drop `.txt` files -> auto-index -> HTTP query endpoint.

## Prerequisites

- [Ollama](https://ollama.ai) running at `localhost:11434` with `embeddinggemma` model
- [Qdrant](https://qdrant.tech) running at `localhost:6334`

## Run

```bash
cargo run -p rag-file
```

## Usage

1. Drop `.txt` files into the printed `docs/` directory
2. Ask questions: `curl -d "your question" http://localhost:8080/ask`

Note: Re-running will re-index all files. For deduplication, clear the Qdrant collection first.
