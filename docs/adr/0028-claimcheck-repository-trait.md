# ADR-0028: Claim Check Repository Trait Boundary

**Date:** 2026-06-27
**Status:** Accepted (Phase 2)
**References:** ADR-0022, ADR-0023

## Decision

### Separate `ClaimCheckRepository` trait (not on `IdempotentRepository`)

Claim Check EIP stashes large message payloads by key so the Exchange carries only a lightweight reference. This is a payload-bearing pattern (`set`/`get` with `Body` values), structurally distinct from the Idempotent Consumer (key-only `contains`/`add`). Each pattern owns its own trait; the shared `NamedRegistry<T>` wiring pattern is cross-referenced but not inherited.

File: `crates/camel-api/src/claim_check.rs:16-52`

```rust
#[async_trait]
pub trait ClaimCheckRepository: Send + Sync + std::fmt::Debug + 'static {
    fn name(&self) -> &str;
    async fn set(&self, key: &str, payload: Body) -> Result<(), CamelError>;
    async fn get(&self, key: &str) -> Result<Body, CamelError>;
    async fn get_and_remove(&self, key: &str) -> Result<Body, CamelError>;
    async fn remove(&self, key: &str) -> Result<(), CamelError>;
    async fn push(&self, key: &str, payload: Body) -> Result<(), CamelError>;
    async fn pop(&self, key: &str) -> Result<Body, CamelError>;
}
```

### Payload type: `Body` (NOT `bytes::Bytes`)

The trait uses `camel_api::Body` as its payload type, preserving `Text`, `Json`, `Xml`, and `Stream` variants alongside `Bytes`. Callers who need raw bytes materialize via `Body::into_bytes()` or `Body::materialize()`.

File: `crates/camel-api/src/claim_check.rs:13` (import), `crates/camel-api/src/body.rs:162` (Body enum).

### Stack operations: `push` / `pop`

The trait includes LIFO stack operations (`push`, `pop`) so Claim Check can be used with the Push/Pop EIP variant without a second trait. The stack is key-scoped (`push("stack-key", body)`). Separate from the single-value key space (`set("key", body)`).

File: `crates/camel-api/src/claim_check.rs:47-52`.

### Contract: `get_and_remove` is atomic

`get_and_remove` returns and deletes in one atomic step. This mirrors the Claim Check's "claim once" semantics — after checkout the payload is released. Implementations MUST ensure no concurrent reader can observe the payload after `get_and_remove` returns.

File: `crates/camel-api/src/claim_check.rs:39-43`.

### `remove` is idempotent

`remove` succeeds even if the key does not exist (no-op). Matches `IdempotentRepository::remove` contract.

File: `crates/camel-api/src/claim_check.rs:45`, `crates/camel-api/src/idempotent.rs:36`.

### Memory implementation: `DashMap` + `VecDeque`

`MemoryClaimCheckRepository` uses `DashMap<String, Body>` for single-value keys and `DashMap<String, VecDeque<Body>>` for LIFO stacks. Interior mutability allows `&self` for all trait methods, safe for concurrent `Arc<dyn ClaimCheckRepository>` sharing.

File: `crates/camel-core/src/claim_check/memory_repository.rs:17-20`.

### Registration: `NamedRegistry<T>` reuse

The Claim Check registry reuses Phase 1's `NamedRegistry<T>` (Mutex-based, duplicate-detecting) with `ClaimCheckRegistry` and `SharedClaimCheckRegistry` type aliases. Pattern identical to `IdempotentRegistry`/`SharedIdempotentRegistry`.

File: `crates/camel-core/src/registry.rs:90-98`.

### CamelContext wiring

`CamelContext` exposes `register_claim_check_repository()` and `claim_check_repository()` methods, mirroring the idempotent repository API. The builder registers a default `"memory"` repository.

File: `crates/camel-core/src/context.rs:700-719`, `crates/camel-core/src/context_builder.rs:248-256`.

### Lifetime threading

`CompilationContext` carries `claim_check_repositories: &'a ClaimCheckRegistry` for future step compilers. No step compiler uses it yet (warning suppressed); the field exists for the upcoming Claim Check DSL step compiler.

File: `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs:105-106`.

### Does NOT implement `StepLifecycle`

Like `MemoryIdempotentRepository`, `MemoryClaimCheckRepository` holds only `Arc`-shared DashMaps with no background work (timers, buckets, queues). No `StepLifecycle` implementation needed. If a persistent backend (Redis, SQL) requires connection lifecycle management, it should implement `StepLifecycle` on the backend client, not the repository wrapper.

File: `crates/camel-api/src/step_lifecycle.rs:31` (StepLifecycle trait).

### `Body` is Clone, stored directly

`Body` implements `Clone` (file: `crates/camel-api/src/body.rs:178-189`). The memory repository stores `Body` values directly (not `Arc<Body>`). This is safe because `Body` variants clone cheaply: `Bytes` (Arc-backed copy-on-write), `Text`/`Xml` (String clone), `Json` (serde_json::Value clone), `Stream` (Arc<Mutex<Option<...>>> clone — shared stream handle).

### Filter deferred

Phase 2 implements whole-body store/retrieve only. Partial-body filter (selectively stashing certain exchange properties/headers while keeping others inline) is deferred to a follow-up issue (bd rc-7qf). The current API is additive — `set`/`get` with full `Body` — and does not preclude a future `FilteredClaimCheckRepository` wrapper.

### `CamelError::RouteError` for "not found"

The memory repository returns `CamelError::RouteError(format!("..."))` for missing-key lookups and empty-stack pops. No new `CamelError` variant is introduced.

File: `crates/camel-core/src/claim_check/memory_repository.rs:55-56`, `crates/camel-core/src/claim_check/memory_repository.rs:88`.

## References

- ADR-0022: StepLifecycle trait and drain — `docs/adr/0022-steplifecycle-trait-and-drain.md`
- ADR-0023: Idempotent Repository trait (structural parent) — `docs/adr/0023-idempotent-repository-trait.md`
- ADR-0024: PipelineOutcome replaces CamelError::Stopped — `docs/adr/0024-pipeline-outcome-replaces-camel-error-stopped.md`
- `NamedRegistry<T>`: `crates/camel-core/src/registry.rs:33-34`
- `IdempotentRegistry` alias: `crates/camel-core/src/registry.rs:77-78`
- `Body` enum: `crates/camel-api/src/body.rs:162-176`
- ClaimCheck EIP spec: Enterprise Integration Patterns (Gregor Hohpe, 2004), Chapter 10
