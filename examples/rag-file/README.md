# RAG Knowledge Base — File-powered Q&A

AcmeCorp's support team uses this demo to query their internal knowledge base. Drop `.txt` documents into `kb/`, they are auto-indexed into a vector store, and an HTTP endpoint answers questions using RAG (Retrieval-Augmented Generation).

## Prerequisites

- [Ollama](https://ollama.ai) running at `localhost:11434`
  - Pull models: `ollama pull embeddinggemma` and `ollama pull qwen3.5:4b`
- [Qdrant](https://qdrant.tech) running at `localhost:6334`

## Run

```bash
docker compose up -d qdrant
cargo run -p rag-file
```

Wait for the documents to be indexed (watch the log output), then ask questions.

## Example Queries

```bash
curl -d "How do I reset a customer's password?" http://localhost:8080/ask
curl -d "What's the refund policy for duplicate charges?" http://localhost:8080/ask
curl -d "How to fix upload error UPLOAD_ERR_7742?" http://localhost:8080/ask
curl -d "What are the API rate limits?" http://localhost:8080/ask
curl -d "How do I configure SSO?" http://localhost:8080/ask
```

### Expected Output

```
To reset a customer's password:
1. Verify the customer's email address in the system
2. Click "Forgot Password" on the login page
3. Enter the registered email address
4. The reset link expires after 24 hours
If the account is locked after 5 failed attempts, a support admin must unlock it manually.
```

## How It Works

```
kb/*.txt ──► File Component (watches directory)
                    │
                    ▼
            Embedding (Ollama embeddinggemma)
                    │
                    ▼
            Vector Upsert (Qdrant)
                    │
      ┌─────────────┘
      │
HTTP /ask ──► Embed query (Ollama embeddinggemma)
                    │
                    ▼
            Vector Search (Qdrant, top 3 results)
                    │
                    ▼
            Prompt Template (question + retrieved context)
                    │
                    ▼
            LLM (Ollama qwen3.5:4b) ──► Answer
```

1. **Ingestion route** watches `kb/` for new `.txt` files, generates embeddings, and upserts them into Qdrant
2. **Query route** accepts HTTP POST at `/ask`, embeds the question, searches Qdrant for relevant documents, builds a prompt with context, and calls the LLM for an answer

## Knowledge Base Documents

The `kb/` directory contains these AcmeCorp support documents:

| File | Topic |
|------|-------|
| `password-reset.txt` | Password reset and locked account procedures |
| `billing-refund.txt` | Refund eligibility and processing steps |
| `app-troubleshooting.txt` | Common application errors and fixes |
| `api-rate-limits.txt` | Rate limiting tiers and best practices |
| `account-management.txt` | Account types, upgrades, SSO configuration |

Add your own `.txt` files to expand the knowledge base — they will be indexed automatically on the next startup.
