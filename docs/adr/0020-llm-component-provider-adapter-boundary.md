# ADR-0020: LLM Component Provider Adapter Boundary

**Date:** 2026-06-13
**Status:** Accepted

## Decision

Define a project-owned `LlmProvider` trait with Camel-shaped request/response types. Confine all production imports of the historical `siumai` SDK to the adapter file (`provider/siumai_adapter.rs`) plus one **test-only** file (`provider/siumai_adapter_tests.rs`, gated by `#[cfg(all(test, feature = "openai"))]`). `provider_factory.rs` selects and dispatches to adapter constructors without importing the SDK directly.

The original boundary included two production files. Refactoring `provider_factory.rs` to delegate narrowed it to one production file without changing the architectural decision.

Test fixtures legitimately need the SDK's types (`StubChat`, `StubEmbed` implement its traits) and cannot be expressed through the public adapter API.

No other file in the crate may import that SDK. A test (`tests/boundary.rs`) scans all `.rs` files for direct SDK references outside its allowlist. The allowlist still permits `provider_factory.rs` for historical compatibility, although that file has no direct imports.

## Context

The `camel-component-llm` component needs an LLM provider abstraction to support multiple backends (OpenAI, Ollama, etc.). Two approaches were considered:

1. **Depend on the SDK types directly** — Similar to how `camel-sql` depends on `sqlx` types directly in its producer.
2. **Define a project-owned `LlmProvider` trait** — Isolate the SDK behind a strict adapter boundary.

The SQL precedent (direct sqlx dependency) does not transfer because:

- `sqlx` is mature (0.8 stable) with a stable API surface.
- `siumai` is `0.11.0-beta.9` — beta quality, API may churn.
- Database concepts (connection, pool, query) are stable and well-understood.
- LLM API semantics (chat, streaming, tool calling, structured output) are still rapidly evolving.

## Considered Options

### Depend on the SDK types directly

Rejected. The beta status of siumai (0.11.0-beta.9) means API churn is likely. Direct dependency would spread SDK types across the component, producer, endpoint, and config — making every breaking change a crate-wide refactor. The SQL precedent does not apply because sqlx is stable and its domain concepts are well-established.

### Define a project-owned trait (Accepted)

Accepted. A project-owned `LlmProvider` trait with Camel-shaped types isolates production SDK imports to one adapter file. Mock provider works without the SDK at all (`--features mock` only). If the SDK breaks, only the adapter file changes. Future providers on a different SDK are possible without breaking the component API.

## Consequences

**Positive:**

- SDK API churn is confined to one production adapter file.
- Mock provider works without the SDK dependency (`--features mock` only).
- Testing is deterministic without network.
- Future providers on a different SDK are possible without breaking the component API.
- Hot-reload, multi-context, and test isolation are safe (own provider map, not global registry).

**Negative:**

- Boilerplate: own request/response types that don't mirror the SDK.
- Manual translation between Camel types and SDK types in the adapter.

**Failure mode:** If production SDK imports or SDK-shaped public types leak past `provider/genai_adapter.rs`, the design has failed.

## Amendment 2026-10-19 (llm-genai-adapter)

The boundary decision STANDS. The SDK behind it changes: siumai 0.11.0-beta.9
→ genai =0.6.5 (stable, not the 0.7.0-rc). Rationale: the historical siumai
beta.10 removed the capability traits the adapter imported (ChatCapability,
EmbeddingCapability, EmbeddingExtensions) and the Ollama provider entirely
(historical siumai-provider-ollama stops at beta.9), and raised MSRV to 1.95
while the workspace declares 1.89 — beta.9 was a dead branch. genai 0.6.5
covers chat_stream + embed, is lighter (≈42 net-new unique packages over the
mock baseline by `cargo tree` at swap time; the siumai comparison figure was
spike-era and is not reproducible post-removal), and accepts our
hardened DNS-pinned reqwest client via `with_reqwest`. The boundary allowlist
narrows to `provider/genai_adapter.rs` plus the adapter tests
(`provider_factory.rs` removed — it imports no SDK); the boundary test now
scans for `genai::` / `use genai`.
