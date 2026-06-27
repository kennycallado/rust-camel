# ADR-0023: Idempotent Repository Trait Boundary

**Date:** 2026-06-27
**Status:** Accepted (Phase 1)
**References:** ADR-0024, ADR-0025
**Related:** Phase 1 — Tasks 2, 3 (Idempotent Consumer EIP)

## Decision

### `IdempotentRepository` trait in `camel-api` (key-only, `Result`-returning)

The `IdempotentRepository` trait lives in `camel-api` (`crates/camel-api/src/idempotent.rs:13`) so any crate (component, processor, test) can implement it without depending on `camel-core` lifecycle internals. This mirrors `StepLifecycle` placement in `camel-api` (ADR-0022 §Trait location).

**Contract C1:** `contains()` returns `Result<bool, CamelError>` (NOT `bool`). Backends (Redis, JDBC, S3) have transient read failures. The Idempotent Consumer propagates `Err` — it must never treat a failed read as "not a duplicate" (`crates/camel-api/src/idempotent.rs:32-36`).

**Key-only:** The trait stores keys (`String`), not full messages. Motivation: an idempotent repository tracks which messages have been seen, not what they contained. Storing full messages would blow memory/backing-store and is the job of a different pattern (Claim Check, Phase 2). Future: a `key_fn: Arc<dyn Fn(&Exchange) -> String>` on the Idempotent Consumer step derives the key from the exchange (e.g. `exchange.message_id()`, header-based, body-hash).

```rust
// File: crates/camel-api/src/idempotent.rs:14-46
#[async_trait]
pub trait IdempotentRepository: Send + Sync + Debug + 'static {
    fn name(&self) -> &str;
    async fn contains(&self, key: &str) -> Result<bool, CamelError>;
    async fn add(&self, key: &str) -> Result<bool, CamelError>;
    async fn remove(&self, key: &str) -> Result<(), CamelError>;
    async fn clear(&self) -> Result<(), CamelError>;
}
```

### `MemoryIdempotentRepository` in `camel-core` (DashMap-backed)

The in-memory implementation uses `DashMap<String, ()>` for concurrent read/write access without a coarse lock (`crates/camel-core/src/idempotent/memory_repository.rs:24`). All trait methods take `&self`, so the repository can be shared via `Arc<dyn IdempotentRepository>` across pipeline steps.

`add()` uses `DashMap::insert` and returns `Ok(!was_present)` — `None` from insert means the key was new (`memory_repository.rs:43`).

Registered as the default `"memory"` repository during `CamelContextBuilder::build()` (`crates/camel-core/src/context_builder.rs:305-311`).

### `NamedRegistry<T>` in `camel-core` (Mutex-based, fallible register)

A generic named-object registry with duplicate detection. Unlike the auth crate's `NamedRegistry` (`crates/services/camel-auth/src/registry.rs:23`, DashMap-based, infallible `register()`), this registry uses `Mutex<HashMap<String, Arc<T>>>` so `register()` can reject duplicates with `Err(RegistryError::AlreadyRegistered)` (`crates/camel-core/src/registry.rs:10-54`).

Type alias: `IdempotentRegistry = NamedRegistry<dyn IdempotentRepository>` (`crates/camel-core/src/registry.rs:79`).

The auth crate's DashMap version overwrites silently — appropriate for security policies where last-writer-wins is acceptable. The Phase 1 registry rejects duplicates because idempotent repository registration is a programming error (two repos with the same name would cause unpredictable routing). If an override is needed, the caller must first remove the existing registration (not yet exposed — future work).

### Segment-not-Process decision for Idempotent Consumer (Task 3)

The Idempotent Consumer EIP (Task 3) MUST use `OutcomeSegment` (not `BoxProcessor`/Process mode). Rationale:

`compose_pipeline` (`crates/camel-core/src/lifecycle/adapters/route_compiler.rs:35`) converts `PipelineOutcome::Stopped` to `Ok(ex)` via `into_tower_result()`. If the Idempotent Consumer ran as a `BoxProcessor` (Process mode), a duplicate-detected `Stopped(ex)` would become `Ok(ex)` — indistinguishable from a successful first-time pass. Downstream steps would continue processing a duplicate exchange.

Using `OutcomeSegment` (`compose_outcome_segment` via `crates/camel-core/src/lifecycle/adapters/outcome_composition.rs`) preserves `PipelineOutcome::Stopped` across the Idempotent Consumer boundary. The sub-pipeline after the duplicate check is skipped (Stopped propagates to `run_steps` which terminates the current branch). This is the core fix for Option E/ADR-0024.

### `RegistryError` visibility

`RegistryError` is `pub` — it's the error type returned by `register_idempotent_repository()`, which is a public method on `CamelContext`. Making `RegistryError` `pub` allows external callers to match on `AlreadyRegistered`.

### Supertrait bounds

`IdempotentRepository` requires `Send + Sync + Debug + 'static`. `Debug` enables logging/tracing of repository identity. `Send + Sync + 'static` are the standard `Arc<dyn ...>` bounds for trait-object storage in registries.

## Context

### Problem

Before Phase 1, the Idempotent Consumer EIP had no pluggable key store. The processor would need to hard-code an in-memory HashSet or HashMap, making it impossible to share state across restarts or cluster nodes. Real deployments need Redis, JDBC, or other backends.

### Key store requirements

- **Pluggable:** Backend must be swappable without changing the EIP processor.
- **Thread-safe:** Multiple pipeline steps may check/add concurrently.
- **Read-failure-transparent (C1):** Transient backend failure must propagate, not silently be treated as "not a duplicate."
- **Key-only:** Duplicate detection checks keys, not full messages.
- **Default memory backend:** Zero-dependency setup for simple cases.
- **Context-scoped:** Repository lives in `CamelContext` so it is available to all routes and lifecycle-managed.

### Why not a single global registry?

Idempotent repository scope is per-route or per-pattern instance, not global. A global registry would require name-scoping at call sites and prevent clean isolation between contexts. `CamelContext`-scoped registration provides the right granularity: the context owns the lifecycle of its repositories.

### Why Mutex, not DashMap for `NamedRegistry`?

DashMap `insert()` always succeeds (overwrites silently). `NamedRegistry` needs fallible `register()` — reject if name is taken. `Mutex<HashMap>` provides atomic check-and-insert. Contention is negligible: registrations happen at context-build time, not per-exchange.

The auth crate's DashMap `NamedRegistry` (`crates/services/camel-auth/src/registry.rs:23`) overwrites silently, which is appropriate for security policies (last-writer-wins for policy evaluation). The two registries coexist for different use cases.

### Phase 2 reuse

`NamedRegistry<T>` will be reused in Phase 2 (Claim Check) with no structural changes — ~3 lines per call site. The generic bound `T: ?Sized + Send + Sync + 'static` accommodates both `dyn IdempotentRepository` and `dyn ClaimCheckStore`.

## Consequences

### Trait location

`IdempotentRepository` in `camel-api` (`idempotent.rs`) means any crate can implement it without depending on `camel-core`. Future backends (Redis in `camel-component-redis`, JDBC in `camel-sql`) can implement the trait remotely.

### Interface stability

The trait has no `#[non_exhaustive]` attribute — adding methods would break existing implementations. Phase 1 considers the 5-method interface stable. If a future backend needs a `len()` or `keys()` method, a separate trait or default method with `unimplemented!()` can be added.

### Default memory backend

`MemoryIdempotentRepository` is registered as `"memory"` in `CamelContextBuilder::build()`. Phase 1 is default-only: `register()` rejects duplicates (`RegistryError::AlreadyRegistered`), so re-registering `"memory"` after `build()` fails. A replace/remove API is future work if override is needed.

### No autowiring

The Idempotent Consumer EIP (Task 3) does not auto-discover repositories by name — the DSL step explicitly names which repository to use. This is intentional: auto-discovery would introduce implicit behavior that breaks when multiple repositories of the same type exist.

### RegistryError is not CamelError

`RegistryError` is a separate enum because it represents a compile-time/configuration error (duplicate name), not a data-plane error. Callers inside `camel-core` can match on `AlreadyRegistered` directly. If a future layer needs to propagate it through Tower, an `Into<CamelError>` impl can be added.

### `PipelineOutcome` interaction (Task 3)

The Idempotent Consumer returns `PipelineOutcome::Stopped(ex)` when a duplicate is detected. This requires `OutcomeSegment` mode because `Stop` → `PipelineOutcome::Stopped` → `Stopped(ex)` is preserved through `run_steps` only for Segment steps. Process steps (BoxProcessor) go through `into_tower_result()` which maps `Stopped` → `Ok(ex)` — losing the semantic distinction. See ADR-0024 §3 for the Tower outcome boundary.

### Phase 1 boundary

Phase 1 Tasks 2-4 implement:
- `IdempotentRepository` trait + `MemoryIdempotentRepository` (this ADR).
- `NamedRegistry<T>` + `IdempotentRegistry` + `CamelContext` wiring (this ADR).
- Idempotent Consumer EIP processor + DSL + test (Task 3).
- ADR-0023 (this document).

What Phase 1 does NOT do:
- Implement Redis/Jdbc backends (future phases).
- Add `len()`, `keys()`, or pagination to the trait.
- Add autowiring or repository discovery.
- Implement Claim Check (Phase 2).

## Load-bearing citations

| File:line | Element |
|---|---|
| `camel-api/src/idempotent.rs:13` | `pub trait IdempotentRepository` |
| `camel-api/src/idempotent.rs:32-36` | Contract C1: `contains` returns `Result<bool, CamelError>` |
| `camel-core/src/idempotent/memory_repository.rs:24` | `struct MemoryIdempotentRepository` (DashMap-backed) |
| `camel-core/src/idempotent/memory_repository.rs:43` | `add()` via `DashMap::insert`, returns `Ok(!was_present)` |
| `camel-core/src/registry.rs:10-54` | `struct NamedRegistry<T>` (Mutex-based, fallible register) |
| `camel-core/src/registry.rs:79` | `type IdempotentRegistry = NamedRegistry<dyn IdempotentRepository>` |
| `camel-core/src/context.rs:27-28` | `idempotent_repositories: IdempotentRegistry` field |
| `camel-core/src/context.rs:329-342` | `register_idempotent_repository()` / `idempotent_repository()` methods |
| `camel-core/src/context_builder.rs:305-311` | Default `"memory"` repository registration in `build()` |
| `camel-core/src/lifecycle/adapters/route_compiler.rs:35` | `compose_pipeline` — `into_tower_result()` maps `Stopped→Ok` |
| `camel-core/src/lifecycle/adapters/outcome_composition.rs` | `compose_outcome_segment` — preserves `Stopped` across Segment |
| `camel-api/src/step_lifecycle.rs:31` | Parallel: `StepLifecycle` in `camel-api` (ADR-0022) |
| `crates/services/camel-auth/src/registry.rs:23` | Auth `NamedRegistry` — infallible `register()` overwrites |
