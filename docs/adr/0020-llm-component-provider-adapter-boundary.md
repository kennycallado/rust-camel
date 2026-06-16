# ADR-0020: LLM Component Provider Adapter Boundary

**Date:** 2026-06-13
**Status:** Accepted

## Decision

Define a project-owned `LlmProvider` trait with Camel-shaped request/response types. Confine all siumai imports to **exactly two production files** (`provider/siumai_adapter.rs` and `provider_factory.rs`) plus one **test-only** file (`provider/siumai_adapter_tests.rs`, gated by `#[cfg(all(test, feature = "openai"))]`).

Test fixtures legitimately need siumai types (`StubChat`, `StubEmbed` implement siumai traits) and cannot be expressed through the public adapter API.

No other file in the crate may import siumai. A test (`tests/boundary.rs`) enforces this by scanning all `.rs` files for `use siumai` or `siumai::` references outside the allowed files.

## Context

The `camel-component-llm` component needs an LLM provider abstraction to support multiple backends (OpenAI, Ollama, etc.). Two approaches were considered:

1. **Depend on siumai types directly** — Similar to how `camel-sql` depends on `sqlx` types directly in its producer.
2. **Define a project-owned `LlmProvider` trait** — Isolate siumai behind a strict adapter boundary.

The SQL precedent (direct sqlx dependency) does not transfer because:

- `sqlx` is mature (0.8 stable) with a stable API surface.
- `siumai` is `0.11.0-beta.9` — beta quality, API may churn.
- Database concepts (connection, pool, query) are stable and well-understood.
- LLM API semantics (chat, streaming, tool calling, structured output) are still rapidly evolving.

## Considered Options

### Depend on siumai types directly

Rejected. The beta status of siumai (0.11.0-beta.9) means API churn is likely. Direct dependency would spread siumai types across the component, producer, endpoint, and config — making every breaking change a crate-wide refactor. The SQL precedent does not apply because sqlx is stable and its domain concepts are well-established.

### Define a project-owned trait (Accepted)

Accepted. A project-owned `LlmProvider` trait with Camel-shaped types isolates siumai to two files. Mock provider works without siumai at all (`--features mock` only). If siumai breaks, only the adapter file changes. Future non-siumai providers are possible without breaking the component API.

## Consequences

**Positive:**

- siumai API churn is confined to two files.
- Mock provider works without siumai dependency (`--features mock` only).
- Testing is deterministic without network.
- Future non-siumai providers are possible without breaking the component API.
- Hot-reload, multi-context, and test isolation are safe (own provider map, not global registry).

**Negative:**

- Boilerplate: own request/response types that don't mirror siumai.
- Manual translation between Camel types and siumai types in the adapter.

**Failure mode:** If siumai API churn leaks past `provider_factory.rs`, the design has failed.
